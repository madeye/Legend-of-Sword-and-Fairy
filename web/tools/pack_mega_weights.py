#!/usr/bin/env python3
"""Pack realesr-animevideov3-fp16.onnx into flat GPU-ready weight blobs.

Layout consumed by the WGSL mega kernel in web/nn-mega.html:

  [mats section]  one mat4x4<f16> (32 bytes, column-major) per
                  (layer, tap, in_vec4, out_vec4), ordered
                  layer -> tap(9) -> iv -> ov.
                  mats[t][iv][ov] column c = weights for input channel iv*4+c,
                  rows = output channels ov*4+0..3.  Input channels of the
                  first conv (3) are zero-padded to 4.
  [vecs section]  per layer: bias (OV vec4<f16>), then PReLU slopes
                  (OV vec4<f16>, absent for the last conv).

The fp16 manifest JSON gives, per layer, matBase (mat4x4 units from buffer
start) and biasBase/slopeBase (vec4<f16> units from buffer start).

The QDQ blob stores signed INT8 weights as packed u32 values in
layer -> tap -> input vec4 -> output vec4 -> output lane order.  Each packed
u32 contains the four input-channel weights consumed by dot4I8Packed.  Weight
scales are per output channel; scales, biases, and PReLU slopes are stored as
vec4<f32>.  Convolution results are dequantized for fp32 bias/PReLU, then
requantized in calibrated 16-channel groups for the following convolution.

Usage: python3 web/tools/pack_mega_weights.py [--source model.onnx]
"""
import argparse
import copy
import glob
import json
import os
import sys

import numpy as np
import onnx

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
MODELS = os.path.join(HERE, "..", "models")
SRC = os.path.join(MODELS, "realesr-animevideov3-fp16.onnx")
OUT_BIN = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.bin")
OUT_JSON = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.json")
QDQ_BIN = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.qdq.bin")
QDQ_JSON = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.qdq.json")


def tensor(inits, name):
    t = inits[name]
    assert t.data_type == onnx.TensorProto.FLOAT16, name
    return np.frombuffer(t.raw_data, np.float16).reshape(tuple(t.dims))


def calibrate_activation_scales(model, image_paths, margin, percentile):
    """Collect static QDQ input scales for the 18 convolutions."""
    import onnxruntime as ort
    from PIL import Image

    shaped = onnx.shape_inference.infer_shapes(copy.deepcopy(model))
    names = [n.output[0] for n in shaped.graph.node if n.op_type == "PRelu"]
    assert len(names) == 17, len(names)
    infos = {x.name: x for x in list(shaped.graph.value_info) +
             list(shaped.graph.output)}
    del shaped.graph.output[:]
    for name in names:
        shaped.graph.output.append(copy.deepcopy(infos[name]))
    session = ort.InferenceSession(shaped.SerializeToString(),
                                   providers=["CPUExecutionProvider"])
    maxima = np.zeros((len(names), 4), np.float32)
    for path in image_paths:
        image = Image.open(path).convert("RGB").resize(
            (320, 200), Image.Resampling.NEAREST)
        data = np.asarray(image, np.float32).transpose(2, 0, 1)[None] / 255.0
        outputs = session.run(None, {"input": data})
        grouped = [np.quantile(np.abs(v.reshape(4, -1)), percentile, axis=1)
                   for v in outputs]
        maxima = np.maximum(maxima, grouped)
    assert np.all(maxima > 0), maxima
    return [np.full(4, 1.0 / 127.0, np.float32)] + [
        v.astype(np.float32) * margin / 127.0 for v in maxima]


def pack_qdq(convs, activation_scales):
    """Pack groupwise INT8 weights plus fp32 Q/DQ parameters."""
    packed = []
    layers = []
    q_cursor = 0  # u32 units
    scale_arrays = []

    for w16, _b, _s in convs:
        w = w16.astype(np.float32)
        o, i = w.shape[0], w.shape[1]
        ov, iv = o // 4, (i + 3) // 4

        # Standard per-output-channel QDQ scale.  Keeping the scale constant
        # across taps allows the shader to accumulate the entire convolution
        # in i32 and dequantize only once per output.
        absmax = np.max(np.abs(w), axis=(1, 2, 3))  # [O]
        scales = np.where(absmax > 0, absmax / 127.0, 1.0).astype(np.float32)
        q = np.clip(np.rint(w / scales[:, None, None, None]),
                    -127, 127).astype(np.int8)

        qp = np.zeros((o, iv * 4, 3, 3), np.int8)
        qp[:, :i] = q
        # [tap, iv, ov, output lane, input lane], with the input lane packed
        # into the bytes of one little-endian u32 for dot4I8Packed.
        a = np.zeros((9, iv, ov, 4, 4), np.int8)
        for ky in range(3):
            for kx in range(3):
                blk = qp[:, :, ky, kx].reshape(ov, 4, iv, 4)
                a[ky * 3 + kx] = blk.transpose(2, 0, 1, 3)
        rec = a.transpose(2, 3, 1, 4, 0).reshape(o, iv * 4, 3, 3)
        assert np.array_equal(rec[:, :i], q), "QDQ packing self-test failed"

        words = np.ascontiguousarray(a).view("<u4").reshape(-1)
        layers.append({"quantBase": q_cursor, "OV": ov, "IV": iv})
        q_cursor += words.size
        packed.append(words)

        scale_arrays.append(scales.reshape(ov, 4))

    assert q_cursor % 4 == 0
    vec_cursor = q_cursor // 4  # one vec4<f32> == four u32 words
    vecs = []
    for (w, b, s), meta, scales, activation_scale in zip(
            convs, layers, scale_arrays, activation_scales):
        meta["activationScaleBase"] = vec_cursor
        vecs.append(np.asarray(activation_scale, np.float32))
        vec_cursor += 1
        meta["weightScaleBase"] = vec_cursor
        vecs.append(scales.reshape(-1).astype(np.float32))
        vec_cursor += meta["OV"]
        meta["biasBase"] = vec_cursor
        vecs.append(b.astype(np.float32))
        vec_cursor += meta["OV"]
        if s is not None:
            meta["slopeBase"] = vec_cursor
            vecs.append(s.astype(np.float32))
            vec_cursor += meta["OV"]
        else:
            meta["slopeBase"] = 0

    for i, meta in enumerate(layers):
        meta["outputScaleBase"] = (layers[i + 1]["activationScaleBase"]
                                   if i + 1 < len(layers) else 0)

    qbytes = np.concatenate(packed).astype("<u4", copy=False).tobytes()
    vbytes = np.concatenate(vecs).astype("<f4", copy=False).tobytes()
    blob = qbytes + vbytes
    assert len(blob) == vec_cursor * 16
    with open(QDQ_BIN, "wb") as f:
        f.write(blob)
    with open(QDQ_JSON, "w") as f:
        json.dump({"format": "qdq-int8-dp4a-f32", "layers": layers,
                   "packedWeightWords": q_cursor, "bytes": len(blob)}, f, indent=1)
    print(f"wrote {QDQ_BIN}: {len(blob)} bytes, {q_cursor} packed weights, "
          f"{len(convs)} layers")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", default=SRC, help="source fp16 ONNX model")
    parser.add_argument("--calibration-image", action="append", default=[],
                        help="320x200-representative image; may be repeated")
    parser.add_argument("--calibration-margin", type=float, default=1.0,
                        help="headroom applied to observed activation ranges")
    parser.add_argument("--calibration-percentile", type=float, default=0.995,
                        help="activation percentile used for static QDQ scales")
    args = parser.parse_args()

    model = onnx.load(args.source)
    g = model.graph
    inits = {t.name: t for t in g.initializer}

    # Collect convs in graph order and attach the PReLU slope that follows.
    convs = []  # (weight[O,I,3,3], bias[O], slope[O] or None)
    for n in g.node:
        if n.op_type == "Conv":
            convs.append([tensor(inits, n.input[1]), tensor(inits, n.input[2]), None])
        elif n.op_type == "PRelu":
            convs[-1][2] = tensor(inits, n.input[1]).reshape(-1)
    assert len(convs) == 18, len(convs)

    mats = []
    layers = []
    mat_cursor = 0
    for w, b, s in convs:
        o, i = w.shape[0], w.shape[1]
        ov, iv = o // 4, (i + 3) // 4
        wp = np.zeros((o, iv * 4, 3, 3), np.float16)
        wp[:, :i] = w
        # a[t, iv, ov, col(in), row(out)] -> column-major mat4x4<f16>
        a = np.zeros((9, iv, ov, 4, 4), np.float16)
        for ky in range(3):
            for kx in range(3):
                m = wp[:, :, ky, kx]  # [O, I4]
                blk = m.reshape(ov, 4, iv, 4)  # [ov, row, iv, col]
                a[ky * 3 + kx] = blk.transpose(2, 0, 3, 1)
        # Self-test: reconstruct the original weight from the packed form.
        rec = a.transpose(2, 4, 1, 3, 0).reshape(o, iv * 4, 3, 3)
        assert np.array_equal(rec[:, :i], w), "packing self-test failed"
        layers.append({"matBase": mat_cursor, "OV": ov, "IV": iv})
        mat_cursor += 9 * iv * ov
        mats.append(a.reshape(-1))

    vec_cursor = mat_cursor * 4  # 1 mat4x4<f16> == 4 vec4<f16>
    vecs = []
    for (w, b, s), meta in zip(convs, layers):
        ov = meta["OV"]
        meta["biasBase"] = vec_cursor
        vecs.append(b.astype(np.float16))
        vec_cursor += ov
        if s is not None:
            meta["slopeBase"] = vec_cursor
            vecs.append(s.astype(np.float16))
            vec_cursor += ov
        else:
            meta["slopeBase"] = 0

    blob = np.concatenate(mats + vecs)
    assert blob.size == vec_cursor * 4
    blob.tofile(OUT_BIN)
    with open(OUT_JSON, "w") as f:
        json.dump({"layers": layers, "totalMats": mat_cursor,
                   "bytes": blob.size * 2}, f, indent=1)
    print(f"wrote {OUT_BIN}: {blob.size * 2} bytes, {mat_cursor} mats, "
          f"{len(convs)} layers")
    calibration_images = args.calibration_image or sorted(
        glob.glob(os.path.join(ROOT, "screenshots", "*.png")))
    if not calibration_images:
        parser.error("at least one --calibration-image is required for QDQ packing")
    activation_scales = calibrate_activation_scales(
        model, calibration_images, args.calibration_margin,
        args.calibration_percentile)
    pack_qdq(convs, activation_scales)


if __name__ == "__main__":
    sys.exit(main())
