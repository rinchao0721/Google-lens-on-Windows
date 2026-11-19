//! 样例图端到端验证，需要真实的 onnxruntime.dll 与样例图，默认忽略。
//!
//! 运行：`LENSWIN_OCR_SAMPLES=<含 sheet.png、lyrics.png 的目录> cargo test -p lenswin-ocr -- --ignored --nocapture`。
//! `LENSWIN_OCR_DLL` 可覆盖 DLL 路径，默认用仓库里的 `assets/ocr/onnxruntime.dll`。

// 辅助函数放进 cfg(test) 模块，才享有 clippy.toml 对测试代码的放宽
#[cfg(test)]
mod support {
    use std::fs::File;
    use std::io::BufReader;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use lenswin_core::frame::CroppedImage;
    use lenswin_ocr::OcrEngine;

    pub fn dll_path() -> PathBuf {
        std::env::var_os("LENSWIN_OCR_DLL")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ocr/onnxruntime.dll")
            })
    }

    fn sample_dir() -> PathBuf {
        PathBuf::from(
            std::env::var_os("LENSWIN_OCR_SAMPLES").expect("需要设置 LENSWIN_OCR_SAMPLES"),
        )
    }

    fn load_png(path: &Path) -> CroppedImage {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path).expect("打开样例图")));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().expect("读取 PNG 头");
        let mut buf = vec![0; reader.output_buffer_size().expect("缓冲区大小")];
        let info = reader.next_frame(&mut buf).expect("解码 PNG");
        let pixels = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => pixels.to_vec(),
            png::ColorType::Rgb => pixels
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [r, g, b, 255])
                .collect(),
            png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => pixels
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[g, a]| [g, g, g, a])
                .collect(),
            other => panic!("不支持的 PNG 颜色类型：{other:?}"),
        };
        CroppedImage::new(info.width, info.height, rgba).expect("构造图像")
    }

    pub fn run_sample(engine: &OcrEngine, name: &str) -> Vec<String> {
        let image = load_png(&sample_dir().join(name));
        engine.recognize(&image).expect("预热识别");
        let started = Instant::now();
        let mut lines = Vec::new();
        for _ in 0..5 {
            lines = engine.recognize(&image).expect("识别");
        }
        println!(
            "== {name} ({}x{}) 预热后平均 {:.1} ms ==",
            image.width(),
            image.height(),
            started.elapsed().as_secs_f64() * 1000.0 / 5.0
        );
        for line in &lines {
            println!("{line}");
        }
        lines
    }
}

#[test]
#[ignore = "需要 onnxruntime.dll 与样例图"]
fn sample_images_are_recognized_without_noise() {
    let started = std::time::Instant::now();
    let engine = lenswin_ocr::OcrEngine::load(&support::dll_path()).expect("加载引擎");
    println!(
        "引擎加载 {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let sheet = support::run_sample(&engine, "sheet.png").join("\n");
    assert!(sheet.contains("选择识别方式"), "{sheet}");
    assert!(sheet.contains("其他 App"), "{sheet}");
    assert!(sheet.contains("麦克风"), "{sheet}");
    assert!(!sheet.contains('>'), "图标噪声应被过滤：{sheet}");

    let lyrics = support::run_sample(&engine, "lyrics.png").join("\n");
    assert!(lyrics.contains("歌词预览"), "{lyrics}");
    assert!(lyrics.contains("真可笑 这些感情"), "{lyrics}");
    assert!(lyrics.contains("16bit/44.1kHz"), "{lyrics}");
    assert!(!lyrics.contains('∂'), "图标噪声应被过滤：{lyrics}");
}
