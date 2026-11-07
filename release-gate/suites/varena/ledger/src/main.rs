#![allow(unsafe_code)]
#[cfg(target_os = "linux")]
mod cells;

fn main() {
    #[cfg(target_os = "linux")]
    cells::main();
}
