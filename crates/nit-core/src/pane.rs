#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaneId {
    Notes,
    JobOutput,
    Editor,
    Visualizer,
    GateMonitor,
}

const SPECS: [(PaneId, &str); 5] = [
    (PaneId::Notes, "AGENT CHAT"),
    (PaneId::JobOutput, "AGENT OPS"),
    (PaneId::Editor, "EDITOR"),
    (PaneId::Visualizer, "VISUALIZER"),
    (PaneId::GateMonitor, "GATE MONITOR"),
];

impl PaneId {
    pub const ALL: [PaneId; 5] = [SPECS[0].0, SPECS[1].0, SPECS[2].0, SPECS[3].0, SPECS[4].0];

    pub fn title(self) -> &'static str {
        SPECS
            .iter()
            .find(|(id, _)| *id == self)
            .map(|(_, t)| *t)
            .unwrap_or("")
    }
}
