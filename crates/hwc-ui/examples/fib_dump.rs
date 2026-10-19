//! Prints the screens, links and atoms of a `.fib` file (development aid):
//! `fib_dump <file.fib>`.
use hwc_formats::fib::Fib;

fn main() {
    let path = std::env::args().nth(1).expect("usage: fib_dump <file.fib>");
    let fib = Fib::parse(&std::fs::read(&path).expect("read")).expect("parse");
    for s in &fib.screens {
        println!("SCREEN {} flags={:x}", s.name, s.flags);
        for l in &s.links {
            println!("  link {:?} -> {:?} flags={:x}", l.name, l.target, l.flags);
        }
        for a in &s.atoms {
            let r = a.rect;
            println!(
                "  {:?} {:?} f={:x} [{},{} {}x{}] ds={:x?} hk={:?} bmp={:?} data={:?} tab={}",
                a.kind,
                a.name,
                a.flags,
                r.left,
                r.top,
                r.width(),
                r.height(),
                a.draw_style,
                a.hotkeys[0],
                a.bitmap,
                a.data,
                a.tab_stop
            );
        }
    }
}
