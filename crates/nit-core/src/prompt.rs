#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Prompt {
    /// `:q` (or Ctrl-q) with unsaved buffers when nit was launched with a
    /// file path. Y → quit anyway; N / Esc → stay.
    ConfirmQuit,
    /// `:q` on a dirty buffer when nit was launched into a directory.
    /// Y → discard changes and close the active buffer (switch to last
    /// remaining buffer or open NITTree); N / Esc → keep the buffer open.
    ConfirmCloseBuffer,
}
