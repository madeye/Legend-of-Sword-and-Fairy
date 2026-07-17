//! Dump a decompressed YJ_1 MKF chunk to stdout (for verification against
//! the C reference implementation). Usage: yj1dump <mkf-file> <chunk-num>

use std::io::Write;

#[path = "../src/yj.rs"]
mod yj;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: yj1dump <mkf> <chunk>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read mkf");
    let n: usize = args[2].parse().expect("chunk number");
    let first = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let count = (first - 4) >> 2;
    assert!(n < count, "chunk out of range");
    let off = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
    let chunk = &data[off(n)..off(n + 1)];
    if !yj::is_yj1(chunk) {
        std::process::exit(1);
    }
    let out = match yj::yj1_decompress(chunk) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("decompress failed: {e}");
            std::process::exit(1);
        }
    };
    std::io::stdout().write_all(&out).unwrap();
}
