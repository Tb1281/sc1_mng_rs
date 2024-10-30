pub use slint::*;

slint::include_modules!();

impl Clone for MainWindow {
    fn clone(&self) -> Self {
        self.clone_strong()
    }
}
