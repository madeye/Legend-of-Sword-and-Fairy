# QDQ / DP4A mega-kernel experiment

Measured on 2026-07-19 with Chrome 150.0.7871.125 and its WebGPU Metal
backend on an Apple M4 (10 GPU cores). Both paths used GPU timestamp queries,
the same `screenshots/scene.png` input, and six measured runs after two warmups.

| Path | Median | Minimum |
| --- | ---: | ---: |
| Shipped FP16 `s4p2` | 26.61 ms | 26.39 ms |
| QDQ INT8 `4x8x8`, 8 pixels/thread | 89.16 ms | 88.71 ms |

The QDQ output measured 41.38 dB RGB PSNR against FP16, with mean absolute
RGB byte error 1.1088, maximum error 96, and 11.012% of RGB samples differing
by more than 2.

The QDQ model is 633,504 bytes, versus 1,244,000 bytes for the FP16 model.

## Backend finding

Chrome was launched with
`--enable-dawn-features=dump_shaders,disable_symbol_renaming`. The emitted MSL
does not contain a native packed integer dot. Each WGSL `dot4I8Packed` is
lowered to shifts and sign extension into `int4`, followed by an ordinary
four-lane integer dot. This lowering explains why topology sweeps from 32 to
256 threads/workgroup and 2 to 16 pixels/thread could not outperform FP16 on
Metal. The best measured INT8 topology remains experimental and is not loaded
by `web/index.html`.

Reproduce the comparison by serving the repository and opening:

```text
http://127.0.0.1:8080/web/nn-tune.html?v=s4p2
```
