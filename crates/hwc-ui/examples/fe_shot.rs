//! Renders front-end screens of any `.fib` straight from the game files (development aid):
//! `fe_shot <data|install|.big> <out dir> <fib name> [screen...]` (all screens when none given).
use std::rc::Rc;

use hwc_formats::fib::Fib;
use hwc_formats::GameData;
use hwc_ui::{AssetSource, Canvas, FrontEnd, Resources};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let src: Rc<dyn AssetSource> = Rc::new(GameData::open(&args[0]).expect("game data"));
    let res = Rc::new(Resources::new(src.clone()));
    let mut fe = FrontEnd::new((), res);
    hwc_ui::cataclysm::register_stubs(&mut fe);
    let fib =
        Fib::parse(&src.read(&format!("FEMan/{}.fib", args[2])).expect("fib")).expect("parse");
    fe.load_fib(&fib);
    let names: Vec<String> = if args.len() > 3 {
        args[3..].to_vec()
    } else {
        fib.screens.iter().map(|s| s.name.clone()).collect()
    };
    let mut canvas = Canvas::new(640, 480);
    for name in &names {
        fe.all_screens_delete();
        fe.screen_start(name);
        fe.process(0.016);
        fe.render(&mut canvas);
        let out = std::path::Path::new(&args[1]).join(format!("{}_{name}.png", args[2]));
        canvas.save_png(&out).unwrap();
        println!("{}", out.display());
    }
}
