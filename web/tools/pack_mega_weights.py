#!/usr/bin/env python3
"""Pack realesr-animevideov3-fp16.onnx into a flat GPU-ready weight blob.

Layout consumed by the WGSL mega kernel in web/nn-mega.html:

  [mats section]  one mat4x4<f16> (32 bytes, column-major) per
                  (layer, tap, in_vec4, out_vec4), ordered
                  layer -> tap(9) -> iv -> ov.
                  mats[t][iv][ov] column c = weights for input channel iv*4+c,
                  rows = output channels ov*4+0..3.  Input channels of the
                  first conv (3) are zero-padded to 4.
  [vecs section]  per layer: bias (OV vec4<f16>), then PReLU slopes
                  (OV vec4<f16>, absent for the last conv).

The manifest JSON gives, per layer, matBase (mat4x4 units from buffer start)
and biasBase/slopeBase (vec4<f16> units from buffer start).

Usage: python3 web/tools/pack_mega_weights.py
"""
import json
import os
import sys

import numpy as np
import onnx

HERE = os.path.dirname(os.path.abspath(__file__))
MODELS = os.path.join(HERE, "..", "models")
SRC = os.path.join(MODELS, "realesr-animevideov3-fp16.onnx")
OUT_BIN = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.bin")
OUT_JSON = os.path.join(MODELS, "realesr-animevideov3-fp16.mega.json")


def tensor(inits, name):
    t = inits[name]
    assert t.data_type == onnx.TensorProto.FLOAT16, name
    return np.frombuffer(t.raw_data, np.float16).reshape(tuple(t.dims))


def main():
    model = onnx.load(SRC)
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


if __name__ == "__main__":
    sys.exit(main())
