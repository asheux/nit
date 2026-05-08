use nit_utils::hashing::stable_hash_bytes;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedSymmetry {
    None,
    MirrorX,
    MirrorY,
    Rotate180,
}

impl SeedSymmetry {
    pub fn next(self) -> Self {
        match self {
            SeedSymmetry::None => SeedSymmetry::MirrorX,
            SeedSymmetry::MirrorX => SeedSymmetry::MirrorY,
            SeedSymmetry::MirrorY => SeedSymmetry::Rotate180,
            SeedSymmetry::Rotate180 => SeedSymmetry::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedSymmetry::None => "none",
            SeedSymmetry::MirrorX => "mirror-x",
            SeedSymmetry::MirrorY => "mirror-y",
            SeedSymmetry::Rotate180 => "rotate-180",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPlacement {
    Center,
    TopLeft,
}

impl SeedPlacement {
    pub fn label(self) -> &'static str {
        match self {
            SeedPlacement::Center => "center",
            SeedPlacement::TopLeft => "top-left",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeedParams {
    pub symmetry: SeedSymmetry,
    pub target_density: f32,
    pub padding: u8,
    pub placement: SeedPlacement,
    pub jitter: f32,
}

impl Default for SeedParams {
    fn default() -> Self {
        Self {
            symmetry: SeedSymmetry::MirrorX,
            target_density: 0.31,
            padding: 1,
            placement: SeedPlacement::Center,
            jitter: 0.04,
        }
    }
}

impl SeedParams {
    pub fn summary(&self) -> String {
        format!(
            "sym:{} dens:{:.2} pad:{} place:{} jit:{:.2}",
            self.symmetry.label(),
            self.target_density,
            self.padding,
            self.placement.label(),
            self.jitter
        )
    }

    // Density and jitter are quantized to micro-units before hashing so that
    // visually identical params with float drift still collapse to the same
    // fingerprint.
    pub fn fingerprint(&self) -> u64 {
        let mut bytes = Vec::with_capacity(16);
        bytes.push(self.symmetry as u8);
        bytes.push(self.placement as u8);
        bytes.extend_from_slice(
            &(self.target_density.clamp(0.0, 1.0) * 1_000_000.0)
                .round()
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &(self.jitter.clamp(0.0, 1.0) * 1_000_000.0)
                .round()
                .to_le_bytes(),
        );
        bytes.push(self.padding);
        stable_hash_bytes(&bytes)
    }
}
