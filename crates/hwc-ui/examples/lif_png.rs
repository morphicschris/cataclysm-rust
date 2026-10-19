//! Converts `.lif` images to PNG (development aid): `lif_png <out dir> <file.lif>...`.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for f in &args[1..] {
        let img = hwc_ui::assets::decode_lif(&std::fs::read(f).expect("read")).expect("lif");
        let mut c = hwc_ui::Canvas::new(img.width, img.height);
        c.clear([255, 0, 255, 255]);
        c.blit(&img, 0, 0, hwc_ui::canvas::BlitMode::Blend);
        let name = std::path::Path::new(f)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        c.save_png(&std::path::Path::new(&args[0]).join(format!("{name}.png")))
            .unwrap();
    }
}
