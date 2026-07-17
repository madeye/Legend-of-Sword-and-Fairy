// AudioWorkletProcessor draining the PAL_AUDIO SharedArrayBuffer ring that
// the (blocked) engine worker tops up from Engine::process_event.
// Layout: Int32Array[2] header (monotonic write/read frame counters,
// wrapping i32) + Float32Array ring of interleaved stereo frames
// (power-of-two frame count).
class PalAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const sab = options.processorOptions.sab;
    this.header = new Int32Array(sab, 0, 2);
    this.ring = new Float32Array(sab, 8);
    this.mask = this.ring.length / 2 - 1;
  }

  process(inputs, outputs) {
    const out = outputs[0];
    const left = out[0];
    const right = out.length > 1 ? out[1] : out[0];
    const frames = left.length;

    const write = Atomics.load(this.header, 0);
    const read = Atomics.load(this.header, 1);
    if (((write - read) | 0) >= frames) {
      for (let i = 0; i < frames; i++) {
        const s = ((read + i) & this.mask) * 2;
        left[i] = this.ring[s];
        right[i] = this.ring[s + 1];
      }
      Atomics.store(this.header, 1, (read + frames) | 0);
    } else {
      left.fill(0);
      right.fill(0);
    }
    return true;
  }
}

registerProcessor("pal-audio", PalAudioProcessor);
