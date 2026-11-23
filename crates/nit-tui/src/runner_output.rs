//! Private CLI output files with independent read and write positions.
pub(crate) fn private_output_stream() -> std::io::Result<(std::fs::File, std::fs::File)> {
    let file = tempfile::NamedTempFile::new()?;
    Ok((file.reopen()?, file.reopen()?))
}
