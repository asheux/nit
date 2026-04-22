use clap::ValueEnum;
use nit_core::LabId;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum LabArg {
    Gol,
    Games,
}

impl From<LabArg> for LabId {
    fn from(value: LabArg) -> Self {
        match value {
            LabArg::Gol => LabId::Gol,
            LabArg::Games => LabId::Games,
        }
    }
}
