#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaneId {
    Notes,
    JobOutput,
    Editor,
    Visualizer,
    GateMonitor,
}

impl PaneId {
    pub const ALL: [PaneId; 5] = [
        PaneId::Notes,
        PaneId::JobOutput,
        PaneId::Editor,
        PaneId::Visualizer,
        PaneId::GateMonitor,
    ];

    pub fn title(self) -> &'static str {
        match self {
            PaneId::Notes => "NOTES",
            PaneId::JobOutput => "JOB OUTPUT",
            PaneId::Editor => "EDITOR",
            PaneId::Visualizer => "VISUALIZER",
            PaneId::GateMonitor => "GATE MONITOR",
        }
    }
}
