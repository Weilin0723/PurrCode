//! Model selection popup.

pub struct ModelPicker {
    pub models: Vec<String>,
    pub selected: usize,
}

impl ModelPicker {
    pub fn new(models: Vec<String>) -> Self {
        Self {
            models,
            selected: 0,
        }
    }
}
