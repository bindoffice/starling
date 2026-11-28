#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageFilterMode {
    #[default]
    Off,
    Test,
    Size,
    Resize,
    Crop,
    Rotate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFilterConfig {
    pub mode: ImageFilterMode,
    pub width: Option<String>,
    pub height: Option<String>,
    pub angle: Option<String>,
    pub buffer_size: u64,
    pub jpeg_quality: Option<String>,
    pub webp_quality: Option<String>,
    pub sharpen: Option<String>,
    pub transparency: bool,
    pub interlace: bool,
}

impl Default for ImageFilterConfig {
    fn default() -> Self {
        Self {
            mode: ImageFilterMode::Off,
            width: None,
            height: None,
            angle: None,
            buffer_size: 1024 * 1024,
            jpeg_quality: None,
            webp_quality: None,
            sharpen: None,
            transparency: true,
            interlace: false,
        }
    }
}
