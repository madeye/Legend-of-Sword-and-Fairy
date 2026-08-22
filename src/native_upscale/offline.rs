//! The neural upscaler, run without a window.
//!
//! The game upscales inside its presentation path: `Video::present` uploads the
//! 320x200 frame, encodes the network and blits the result onto the swapchain,
//! all inside one vsync.  A recorder cannot do that — the network costs tens of
//! milliseconds per frame, and the engine clock is wall-clock, so upscaling
//! while the game runs would push the game itself behind real time.
//!
//! So the recorder captures cheap 320x200 frames live and comes back here
//! afterwards, with no deadline, to turn each one into a 1280x800 image.

use super::{
    request_headless_gpu, NativeUpscaler, INPUT_H, INPUT_W, OUTPUT_H, OUTPUT_ROW_BYTES, OUTPUT_W,
};
use pixels::wgpu;

/// Size of the RGBA buffer `upscale` expects to be handed.
pub const INPUT_SIZE: usize = (INPUT_W * INPUT_H * 4) as usize;
/// Dimensions of the image `upscale` produces.
pub const OUTPUT_WIDTH: u32 = OUTPUT_W;
pub const OUTPUT_HEIGHT: u32 = OUTPUT_H;
/// Size of the RGBA buffer `upscale` writes into.
pub const OUTPUT_SIZE: usize = (OUTPUT_W * OUTPUT_H * 4) as usize;

/// A windowless instance of the upscale network: 320x200 RGBA in, 1280x800
/// RGBA out, exactly the 4x the shipped model was trained for.
pub struct OfflineUpscaler {
    device: wgpu::Device,
    queue: wgpu::Queue,
    upscaler: NativeUpscaler,
    readback: wgpu::Buffer,
    adapter_name: String,
}

impl OfflineUpscaler {
    /// Open a GPU and build the network, or `None` when no adapter can run it
    /// (no `SHADER_F16`, no GPU at all).  Callers are expected to fall back to
    /// plain scaling rather than fail.
    pub fn new() -> Option<OfflineUpscaler> {
        let gpu = request_headless_gpu()?;
        // The blit pipeline this builds is unused here — only `encode_network`
        // runs — but it has to name *a* format, so name the one the output
        // texture already uses.
        let upscaler = NativeUpscaler::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offline upscale readback"),
            size: OUTPUT_ROW_BYTES * OUTPUT_H as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(OfflineUpscaler {
            device: gpu.device,
            queue: gpu.queue,
            upscaler,
            readback,
            adapter_name: gpu.adapter_info.name,
        })
    }

    /// The GPU the network is running on, for logging.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Upscale one frame.  `input` is `INPUT_SIZE` bytes of RGBA, `out` is
    /// `OUTPUT_SIZE` bytes of RGBA.  Blocks until the GPU is done.
    pub fn upscale(&self, input: &[u8], out: &mut [u8]) {
        assert_eq!(
            input.len(),
            INPUT_SIZE,
            "upscale input must be 320x200 RGBA"
        );
        assert_eq!(
            out.len(),
            OUTPUT_SIZE,
            "upscale output must be 1280x800 RGBA"
        );

        self.upscaler.upload_input(&self.queue, input);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("offline upscale"),
            });
        self.upscaler.encode_network(&mut encoder);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.upscaler.output_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(OUTPUT_ROW_BYTES as u32),
                    rows_per_image: Some(OUTPUT_H),
                },
            },
            wgpu::Extent3d {
                width: OUTPUT_W,
                height: OUTPUT_H,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map upscale readback"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll upscale readback");
        // `OUTPUT_ROW_BYTES` is 5120, already a multiple of wgpu's 256-byte
        // row alignment, so the mapped bytes are the image with no padding.
        out.copy_from_slice(&slice.get_mapped_range());
        self.readback.unmap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skipped on machines without a usable GPU, which is exactly when
    /// `new` returns `None`.
    #[test]
    fn upscales_a_gradient_without_a_window() {
        let Some(up) = OfflineUpscaler::new() else {
            eprintln!("no SHADER_F16 adapter, skipping");
            return;
        };
        let mut input = vec![0u8; INPUT_SIZE];
        for y in 0..INPUT_H as usize {
            for x in 0..INPUT_W as usize {
                let p = (y * INPUT_W as usize + x) * 4;
                input[p] = (x * 255 / INPUT_W as usize) as u8;
                input[p + 1] = (y * 255 / INPUT_H as usize) as u8;
                input[p + 2] = 128;
                input[p + 3] = 255;
            }
        }
        let mut out = vec![0u8; OUTPUT_SIZE];
        up.upscale(&input, &mut out);

        // The network must have written something with the gradient's shape:
        // dark on the left, bright on the right, opaque throughout.
        let red_at = |x: usize, y: usize| out[(y * OUTPUT_W as usize + x) * 4] as u32;
        assert!(
            red_at(OUTPUT_W as usize - 8, 400) > red_at(8, 400) + 100,
            "output does not follow the input gradient"
        );
        assert!(
            out.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "output not opaque"
        );
    }
}
