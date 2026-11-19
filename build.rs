use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_engine)");
    println!("cargo:rerun-if-env-changed=OC_EMBED_FFMPEG");
    // Single-file build: OC_EMBED_FFMPEG=<path to ffmpeg.exe> packs the engine into the exe (zstd).
    if let Some(src) = std::env::var_os("OC_EMBED_FFMPEG").filter(|s| !s.is_empty()) {
        let src = PathBuf::from(src);
        println!("cargo:rerun-if-changed={}", src.display());
        let data = std::fs::read(&src).expect("OC_EMBED_FFMPEG: cannot read ffmpeg.exe");
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ffmpeg.zst");
        let mut enc = zstd::stream::Encoder::new(Vec::new(), 19).unwrap();
        let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
        enc.multithread(threads).unwrap();
        std::io::Write::write_all(&mut enc, &data).unwrap();
        let packed = enc.finish().unwrap();
        std::fs::write(&out, &packed).unwrap();
        // Identity of this engine build: size + FNV-1a of the bytes.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in &data {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        println!("cargo:rustc-env=OC_ENGINE_ID={:x}", h);
        println!("cargo:rustc-env=OC_ENGINE_SIZE={}", data.len());
        println!("cargo:rustc-cfg=embedded_engine");
    }
    tauri_build::build()
}
