#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaneId {
    FileTree,
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
            PaneId::FileTree => "NITTREE",
            PaneId::Notes => "AGENT CHAT",
            PaneId::JobOutput => "AGENT OPS",
            PaneId::Editor => "EDITOR",
            PaneId::Visualizer => "VISUALIZER",
            PaneId::GateMonitor => "GATE MONITOR",
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StationCenterTab {
    #[default]
    Chat,
    Editor,
    Terminal,
}

impl StationCenterTab {
    pub fn pane(self) -> PaneId {
        match self {
            Self::Chat | Self::Terminal => PaneId::Notes,
            Self::Editor => PaneId::Editor,
        }
    }
}
