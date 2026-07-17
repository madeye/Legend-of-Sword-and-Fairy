//! YJ_1 / YJ_2 decompression. Faithful port of SDLPAL yj1.c.
//!
//! The DOS data set uses YJ_1 everywhere; YJ_2 (used by the WIN95 version)
//! is ported for completeness.
#![allow(dead_code)]

use std::io;

pub const YJ1_SIGNATURE: u32 = 0x315f_4a59; // "YJ_1"

fn err(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Does the buffer carry a YJ_1 signature?
pub fn is_yj1(data: &[u8]) -> bool {
    data.len() >= 4 && u32_le(data, 0) == YJ1_SIGNATURE
}

/// Decompress a buffer if it carries a YJ_1 signature; otherwise return a
/// copy unchanged (uncompressed chunks are common in the DOS data).
pub fn decompress(input: &[u8]) -> io::Result<Vec<u8>> {
    if is_yj1(input) {
        yj1_decompress(input)
    } else {
        Ok(input.to_vec())
    }
}

#[inline]
fn u16_le(b: &[u8], off: usize) -> u16 {
    b[off] as u16 | ((b[off + 1] as u16) << 8)
}

#[inline]
fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Bit reader over 16-bit little-endian words (yj1_get_bits).
#[inline]
fn yj1_get_bits(src: &[u8], bitptr: &mut u32, count: u32) -> io::Result<u32> {
    let base = ((*bitptr >> 4) << 1) as usize;
    let bptr = *bitptr & 0xf;
    if base + 1 >= src.len() {
        return Err(err("YJ_1: bitstream out of bounds"));
    }
    let w0 = u16_le(src, base) as u32;
    *bitptr += count;
    if count > 16 - bptr {
        let count = count + bptr - 16;
        let mask = 0xffffu32 >> bptr;
        if base + 3 >= src.len() {
            return Err(err("YJ_1: bitstream out of bounds"));
        }
        let w1 = u16_le(src, base + 2) as u32;
        Ok(((w0 & mask) << count) | (w1 >> (16 - count)))
    } else {
        // C: ((unsigned short)(w0 << bptr)) >> (16 - count)
        Ok(((w0 << bptr) & 0xffff) >> (16 - count))
    }
}

struct Yj1BlockHeader {
    uncompressed_length: u16,
    compressed_length: u16,
    lzss_repeat_table: [u16; 4],
    lzss_offset_code_length_table: [u8; 4],
    lzss_repeat_code_length_table: [u8; 3],
    code_count_code_length_table: [u8; 3],
    code_count_table: [u8; 2],
}

impl Yj1BlockHeader {
    fn parse(b: &[u8]) -> io::Result<Yj1BlockHeader> {
        if b.len() < 24 {
            return Err(err("YJ_1: truncated block header"));
        }
        Ok(Yj1BlockHeader {
            uncompressed_length: u16_le(b, 0),
            compressed_length: u16_le(b, 2),
            lzss_repeat_table: [u16_le(b, 4), u16_le(b, 6), u16_le(b, 8), u16_le(b, 10)],
            lzss_offset_code_length_table: [b[12], b[13], b[14], b[15]],
            lzss_repeat_code_length_table: [b[16], b[17], b[18]],
            code_count_code_length_table: [b[19], b[20], b[21]],
            code_count_table: [b[22], b[23]],
        })
    }
}

fn yj1_get_loop(src: &[u8], bitptr: &mut u32, header: &Yj1BlockHeader) -> io::Result<u16> {
    if yj1_get_bits(src, bitptr, 1)? != 0 {
        Ok(header.code_count_table[0] as u16)
    } else {
        let temp = yj1_get_bits(src, bitptr, 2)?;
        if temp != 0 {
            Ok(yj1_get_bits(
                src,
                bitptr,
                header.code_count_code_length_table[temp as usize - 1] as u32,
            )? as u16)
        } else {
            Ok(header.code_count_table[1] as u16)
        }
    }
}

fn yj1_get_count(src: &[u8], bitptr: &mut u32, header: &Yj1BlockHeader) -> io::Result<u16> {
    let temp = yj1_get_bits(src, bitptr, 2)?;
    if temp != 0 {
        if yj1_get_bits(src, bitptr, 1)? != 0 {
            Ok(yj1_get_bits(
                src,
                bitptr,
                header.lzss_repeat_code_length_table[temp as usize - 1] as u32,
            )? as u16)
        } else {
            Ok(header.lzss_repeat_table[temp as usize])
        }
    } else {
        Ok(header.lzss_repeat_table[0])
    }
}

/// Huffman tree node (flattened; children referenced by index).
#[derive(Clone, Copy, Default)]
struct Yj1Node {
    value: u8,
    leaf: bool,
    left: u16,
    right: u16,
}

/// Decompress a YJ_1 buffer. The output size comes from the file header.
pub fn yj1_decompress(input: &[u8]) -> io::Result<Vec<u8>> {
    if input.len() < 16 {
        return Err(err("YJ_1: input too short"));
    }
    if u32_le(input, 0) != YJ1_SIGNATURE {
        return Err(err("YJ_1: bad signature"));
    }
    let uncompressed_length = u32_le(input, 4) as usize;
    let block_count = u16_le(input, 12) as usize;
    let tree_len = (input[15] as usize) * 2;

    // Build the Huffman tree. Node values live at input[15 + i] for
    // i in 1..=tree_len; leaf flags are a bitstream right after the values,
    // padded to 16-bit words.
    let flag_off = 16 + tree_len;
    let mut nodes = vec![Yj1Node::default(); tree_len + 1];
    if tree_len < 2 {
        return Err(err("YJ_1: empty huffman tree"));
    }
    nodes[0] = Yj1Node {
        value: 0,
        leaf: false,
        left: 1,
        right: 2,
    };
    {
        let flag = input
            .get(flag_off..)
            .ok_or_else(|| err("YJ_1: truncated tree"))?;
        let mut bitptr: u32 = 0;
        for (i, node) in nodes.iter_mut().enumerate().skip(1) {
            let leaf = yj1_get_bits(flag, &mut bitptr, 1)? == 0;
            let value = *input
                .get(15 + i)
                .ok_or_else(|| err("YJ_1: truncated tree"))?;
            let (left, right) = if leaf {
                (0, 0)
            } else {
                let l = ((value as usize) << 1) + 1;
                if l + 1 > tree_len {
                    return Err(err("YJ_1: tree child out of range"));
                }
                (l as u16, l as u16 + 1)
            };
            *node = Yj1Node {
                value,
                leaf,
                left,
                right,
            };
        }
    }
    // Skip file header + tree values + flag words.
    let flag_words = if tree_len & 0xf != 0 {
        (tree_len >> 4) + 1
    } else {
        tree_len >> 4
    };
    let mut src = 16 + tree_len + flag_words * 2;

    let mut out: Vec<u8> = Vec::with_capacity(uncompressed_length);

    for _ in 0..block_count {
        let block = input
            .get(src..)
            .ok_or_else(|| err("YJ_1: truncated block"))?;
        if block.len() < 4 {
            return Err(err("YJ_1: truncated block header"));
        }
        let block_uncompressed = u16_le(block, 0);
        let block_compressed = u16_le(block, 2);

        if block_compressed == 0 {
            // Raw block: 4-byte header then UncompressedLength raw bytes.
            let n = block_uncompressed as usize;
            let data = input
                .get(src + 4..src + 4 + n)
                .ok_or_else(|| err("YJ_1: truncated raw block"))?;
            out.extend_from_slice(data);
            src += 4 + n;
            continue;
        }

        let header = Yj1BlockHeader::parse(block)?;
        // Compressed block: bitstream starts 24 bytes into the block. Like
        // the C code, let bit reads run to the end of the whole input (the
        // final loop-terminator bits may sit in the last partial word).
        let stream = &input[src + 24..];
        let mut bitptr: u32 = 0;
        loop {
            let mut n = yj1_get_loop(stream, &mut bitptr, &header)?;
            if n == 0 {
                break;
            }
            while n > 0 {
                n -= 1;
                let mut node = 0usize;
                while !nodes[node].leaf {
                    node = if yj1_get_bits(stream, &mut bitptr, 1)? != 0 {
                        nodes[node].right as usize
                    } else {
                        nodes[node].left as usize
                    };
                }
                out.push(nodes[node].value);
            }

            let mut n = yj1_get_loop(stream, &mut bitptr, &header)?;
            if n == 0 {
                break;
            }
            while n > 0 {
                n -= 1;
                let count = yj1_get_count(stream, &mut bitptr, &header)? as usize;
                let pos = yj1_get_bits(stream, &mut bitptr, 2)? as usize;
                let pos = yj1_get_bits(
                    stream,
                    &mut bitptr,
                    header.lzss_offset_code_length_table[pos] as u32,
                )? as usize;
                if pos > out.len() {
                    return Err(err("YJ_1: LZSS offset out of range"));
                }
                for _ in 0..count {
                    if pos == 0 {
                        // C writes *dest = *(dest - 0): a self-assignment,
                        // leaving whatever the (calloc'ed) buffer held.
                        out.push(0);
                    } else {
                        let b = out[out.len() - pos];
                        out.push(b);
                    }
                }
            }
        }
        src += header.compressed_length as usize;
    }

    if out.len() != uncompressed_length {
        return Err(err(&format!(
            "YJ_1: length mismatch (got {}, header says {})",
            out.len(),
            uncompressed_length
        )));
    }
    Ok(out)
}

// ===========================================================================
// YJ_2 (adaptive Huffman + LZSS), used only by the WIN95 data set.
// ===========================================================================

#[rustfmt::skip]
const YJ2_DATA1: [u8; 0x100] = [
    0x3f, 0x0b, 0x17, 0x03, 0x2f, 0x0a, 0x16, 0x00, 0x2e, 0x09, 0x15, 0x02, 0x2d, 0x01, 0x08, 0x00,
    0x3e, 0x07, 0x14, 0x03, 0x2c, 0x06, 0x13, 0x00, 0x2b, 0x05, 0x12, 0x02, 0x2a, 0x01, 0x04, 0x00,
    0x3d, 0x0b, 0x11, 0x03, 0x29, 0x0a, 0x10, 0x00, 0x28, 0x09, 0x0f, 0x02, 0x27, 0x01, 0x08, 0x00,
    0x3c, 0x07, 0x0e, 0x03, 0x26, 0x06, 0x0d, 0x00, 0x25, 0x05, 0x0c, 0x02, 0x24, 0x01, 0x04, 0x00,
    0x3b, 0x0b, 0x17, 0x03, 0x23, 0x0a, 0x16, 0x00, 0x22, 0x09, 0x15, 0x02, 0x21, 0x01, 0x08, 0x00,
    0x3a, 0x07, 0x14, 0x03, 0x20, 0x06, 0x13, 0x00, 0x1f, 0x05, 0x12, 0x02, 0x1e, 0x01, 0x04, 0x00,
    0x39, 0x0b, 0x11, 0x03, 0x1d, 0x0a, 0x10, 0x00, 0x1c, 0x09, 0x0f, 0x02, 0x1b, 0x01, 0x08, 0x00,
    0x38, 0x07, 0x0e, 0x03, 0x1a, 0x06, 0x0d, 0x00, 0x19, 0x05, 0x0c, 0x02, 0x18, 0x01, 0x04, 0x00,
    0x37, 0x0b, 0x17, 0x03, 0x2f, 0x0a, 0x16, 0x00, 0x2e, 0x09, 0x15, 0x02, 0x2d, 0x01, 0x08, 0x00,
    0x36, 0x07, 0x14, 0x03, 0x2c, 0x06, 0x13, 0x00, 0x2b, 0x05, 0x12, 0x02, 0x2a, 0x01, 0x04, 0x00,
    0x35, 0x0b, 0x11, 0x03, 0x29, 0x0a, 0x10, 0x00, 0x28, 0x09, 0x0f, 0x02, 0x27, 0x01, 0x08, 0x00,
    0x34, 0x07, 0x0e, 0x03, 0x26, 0x06, 0x0d, 0x00, 0x25, 0x05, 0x0c, 0x02, 0x24, 0x01, 0x04, 0x00,
    0x33, 0x0b, 0x17, 0x03, 0x23, 0x0a, 0x16, 0x00, 0x22, 0x09, 0x15, 0x02, 0x21, 0x01, 0x08, 0x00,
    0x32, 0x07, 0x14, 0x03, 0x20, 0x06, 0x13, 0x00, 0x1f, 0x05, 0x12, 0x02, 0x1e, 0x01, 0x04, 0x00,
    0x31, 0x0b, 0x11, 0x03, 0x1d, 0x0a, 0x10, 0x00, 0x1c, 0x09, 0x0f, 0x02, 0x1b, 0x01, 0x08, 0x00,
    0x30, 0x07, 0x0e, 0x03, 0x1a, 0x06, 0x0d, 0x00, 0x19, 0x05, 0x0c, 0x02, 0x18, 0x01, 0x04, 0x00,
];
const YJ2_DATA2: [u8; 0x10] = [
    0x08, 0x05, 0x06, 0x04, 0x07, 0x05, 0x06, 0x03, 0x07, 0x05, 0x06, 0x04, 0x07, 0x04, 0x05, 0x03,
];

/// Adaptive Huffman tree for YJ_2, stored as index-based nodes to keep the
/// pointer-juggling of the C code intact.
struct Yj2Tree {
    weight: [u16; 641],
    value: [u16; 641],
    parent: [u16; 641],
    left: [u16; 641],
    right: [u16; 641],
    /// list[value] -> node index, for leaf values 0..=0x140.
    list: [u16; 321],
}

impl Yj2Tree {
    fn new() -> Yj2Tree {
        let mut t = Yj2Tree {
            weight: [0; 641],
            value: [0; 641],
            parent: [0; 641],
            left: [0; 641],
            right: [0; 641],
            list: [0; 321],
        };
        for i in 0..=0x140u16 {
            t.list[i as usize] = i;
        }
        for i in 0..=0x280usize {
            t.value[i] = i as u16;
            t.weight[i] = 1;
        }
        t.parent[0x280] = 0x280;
        let mut i = 0usize;
        for ptr in 0x141..=0x280usize {
            t.left[ptr] = i as u16;
            t.right[ptr] = i as u16 + 1;
            t.parent[i] = ptr as u16;
            t.parent[i + 1] = ptr as u16;
            t.weight[ptr] = t.weight[i] + t.weight[i + 1];
            i += 2;
        }
        t
    }

    /// Port of yj2_adjust_tree. The C code first swaps the two nodes'
    /// parent fields, then swaps the whole structs — the net effect is that
    /// value/weight/left/right are exchanged while each slot keeps its
    /// original parent. Child->parent and list fix-ups happen in between,
    /// based on the pre-swap contents.
    fn adjust(&mut self, value: u16) {
        let mut node = self.list[value as usize] as usize;
        while self.value[node] != 0x280 {
            let mut temp = node + 1;
            while self.weight[node] == self.weight[temp] {
                temp += 1;
            }
            temp -= 1;
            if temp != node {
                if self.value[node] > 0x140 {
                    let (l, r) = (self.left[node] as usize, self.right[node] as usize);
                    self.parent[l] = temp as u16;
                    self.parent[r] = temp as u16;
                } else {
                    self.list[self.value[node] as usize] = temp as u16;
                }
                if self.value[temp] > 0x140 {
                    let (l, r) = (self.left[temp] as usize, self.right[temp] as usize);
                    self.parent[l] = node as u16;
                    self.parent[r] = node as u16;
                } else {
                    self.list[self.value[temp] as usize] = node as u16;
                }
                self.value.swap(node, temp);
                self.weight.swap(node, temp);
                self.left.swap(node, temp);
                self.right.swap(node, temp);
                node = temp;
            }
            self.weight[node] += 1;
            node = self.parent[node] as usize;
        }
        self.weight[node] += 1;
    }
}

#[inline]
fn yj2_bt(data: &[u8], pos: u32) -> io::Result<u32> {
    let byte = data
        .get((pos >> 3) as usize)
        .ok_or_else(|| err("YJ_2: bitstream out of bounds"))?;
    Ok(((byte >> (pos & 0x7)) & 1) as u32)
}

/// Decompress a YJ_2 buffer. The first u32 is the decompressed length.
pub fn yj2_decompress(input: &[u8]) -> io::Result<Vec<u8>> {
    if input.len() < 4 {
        return Err(err("YJ_2: input too short"));
    }
    let length = u32_le(input, 0) as usize;
    let src = &input[4..];
    let mut tree = Yj2Tree::new();
    let mut out: Vec<u8> = Vec::with_capacity(length);
    let mut ptr: u32 = 0;

    loop {
        let mut node = 0x280usize;
        while tree.value[node] > 0x140 {
            node = if yj2_bt(src, ptr)? != 0 {
                tree.right[node] as usize
            } else {
                tree.left[node] as usize
            };
            ptr += 1;
        }
        let val = tree.value[node];
        if tree.weight[0x280] == 0x8000 {
            for i in 0..0x141usize {
                if tree.weight[tree.list[i] as usize] & 1 != 0 {
                    tree.adjust(i as u16);
                }
            }
            for w in tree.weight.iter_mut() {
                *w >>= 1;
            }
        }
        tree.adjust(val);
        if val > 0xff {
            let mut temp: u32 = 0;
            let mut i = 0u32;
            while i < 8 {
                temp |= yj2_bt(src, ptr)? << i;
                i += 1;
                ptr += 1;
            }
            let tmp = temp & 0xff;
            while i < YJ2_DATA2[(tmp & 0xf) as usize] as u32 + 6 {
                temp |= yj2_bt(src, ptr)? << i;
                i += 1;
                ptr += 1;
            }
            temp >>= YJ2_DATA2[(tmp & 0xf) as usize];
            let pos = ((temp & 0x3f) | ((YJ2_DATA1[tmp as usize] as u32) << 6)) as usize;
            if pos == 0xfff {
                break;
            }
            let count = (val - 0xfd) as usize;
            if pos + 1 > out.len() {
                return Err(err("YJ_2: LZSS offset out of range"));
            }
            let start = out.len() - pos - 1;
            for k in 0..count {
                let b = out[start + k];
                out.push(b);
            }
        } else {
            out.push(val as u8);
        }
    }

    if out.len() != length {
        return Err(err(&format!(
            "YJ_2: length mismatch (got {}, header says {})",
            out.len(),
            length
        )));
    }
    Ok(out)
}
