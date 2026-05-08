use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedEncoderId {
    AsciiBytes,
    Lifehash16,
    HilbertBits,
    Structural,
    TokenSpectrum,
    AstStructure,
    ComplexityField,
}

impl SeedEncoderId {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedEncoderId::AsciiBytes => "ascii_bytes",
            SeedEncoderId::Lifehash16 => "lifehash16",
            SeedEncoderId::HilbertBits => "hilbert_bits",
            SeedEncoderId::Structural => "structural",
            SeedEncoderId::TokenSpectrum => "token_spectrum",
            SeedEncoderId::AstStructure => "ast_structure",
            SeedEncoderId::ComplexityField => "complexity_field",
        }
    }

    pub fn label(self) -> &'static str {
        self.as_str()
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "ascii_bytes" => Some(SeedEncoderId::AsciiBytes),
            "lifehash16" => Some(SeedEncoderId::Lifehash16),
            "hilbert_bits" => Some(SeedEncoderId::HilbertBits),
            "structural" => Some(SeedEncoderId::Structural),
            "token_spectrum" => Some(SeedEncoderId::TokenSpectrum),
            "ast_structure" => Some(SeedEncoderId::AstStructure),
            "complexity_field" => Some(SeedEncoderId::ComplexityField),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            SeedEncoderId::AsciiBytes => SeedEncoderId::HilbertBits,
            SeedEncoderId::HilbertBits => SeedEncoderId::Lifehash16,
            SeedEncoderId::Lifehash16 => SeedEncoderId::Structural,
            SeedEncoderId::Structural => SeedEncoderId::TokenSpectrum,
            SeedEncoderId::TokenSpectrum => SeedEncoderId::AstStructure,
            SeedEncoderId::AstStructure => SeedEncoderId::ComplexityField,
            SeedEncoderId::ComplexityField => SeedEncoderId::AsciiBytes,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedViewMode {
    Genome,
    Plate,
    Map,
    Stats,
}

impl SeedViewMode {
    pub fn next(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Map,
            SeedViewMode::Map => SeedViewMode::Stats,
            SeedViewMode::Stats => SeedViewMode::Genome,
        }
    }

    pub fn toggle_plate(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Genome,
            other => other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedViewMode::Genome => "GENOME",
            SeedViewMode::Plate => "PLATE",
            SeedViewMode::Map => "MAP",
            SeedViewMode::Stats => "STATS",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPreviewMode {
    Solid,
    HalfBlock,
    Braille,
    Tissue,
    Heatmap,
}

impl SeedPreviewMode {
    pub fn next(self) -> Self {
        match self {
            SeedPreviewMode::Solid => SeedPreviewMode::HalfBlock,
            SeedPreviewMode::HalfBlock => SeedPreviewMode::Braille,
            SeedPreviewMode::Braille => SeedPreviewMode::Tissue,
            SeedPreviewMode::Tissue => SeedPreviewMode::Heatmap,
            SeedPreviewMode::Heatmap => SeedPreviewMode::Solid,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedPreviewMode::Solid => "SOLID",
            SeedPreviewMode::HalfBlock => "HALF",
            SeedPreviewMode::Braille => "BRAILLE",
            SeedPreviewMode::Tissue => "TISSUE",
            SeedPreviewMode::Heatmap => "HEAT",
        }
    }
}
