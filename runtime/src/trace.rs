use crate::sim::types::SignalValue;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct CsvTraceWriter {
    path: PathBuf,
    writer: std::io::BufWriter<std::fs::File>,
    row_count: u64,
}

impl CsvTraceWriter {
    pub fn create(path: impl AsRef<Path>, headers: &[String]) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create trace output dir: {err}"))?;
        }

        let file = std::fs::File::create(&path)
            .map_err(|err| format!("failed to create trace file '{}': {err}", path.display()))?;
        let mut writer = std::io::BufWriter::new(file);

        writer
            .write_all(b"tick,time_us")
            .map_err(|err| format!("failed to write trace header: {err}"))?;
        for header in headers {
            writer
                .write_all(b",")
                .map_err(|err| format!("failed to write trace header separator: {err}"))?;
            write_csv_cell(&mut writer, header)
                .map_err(|err| format!("failed to write trace header entry '{header}': {err}"))?;
        }
        writer
            .write_all(b"\n")
            .map_err(|err| format!("failed to terminate trace header: {err}"))?;
        writer
            .flush()
            .map_err(|err| format!("failed to flush trace header: {err}"))?;

        Ok(Self {
            path,
            writer,
            row_count: 0,
        })
    }

    pub fn write_row(
        &mut self,
        tick: u64,
        time_us: u64,
        values: &[SignalValue],
    ) -> Result<(), String> {
        write!(self.writer, "{tick},{time_us}")
            .map_err(|err| format!("failed to write trace row prefix: {err}"))?;
        for value in values {
            self.writer
                .write_all(b",")
                .map_err(|err| format!("failed to write trace row separator: {err}"))?;
            write_signal_value(&mut self.writer, value)
                .map_err(|err| format!("failed to write trace row value: {err}"))?;
        }
        self.writer
            .write_all(b"\n")
            .map_err(|err| format!("failed to write trace row newline: {err}"))?;
        self.row_count += 1;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        self.writer
            .flush()
            .map_err(|err| format!("failed to flush trace file: {err}"))
    }

    pub fn row_count(&self) -> u64 {
        self.row_count
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn write_signal_value(writer: &mut impl Write, value: &SignalValue) -> Result<(), std::io::Error> {
    match value {
        SignalValue::Bool(value) => write!(writer, "{value}"),
        SignalValue::U32(value) => write!(writer, "{value}"),
        SignalValue::I32(value) => write!(writer, "{value}"),
        SignalValue::F32(value) => write!(writer, "{value}"),
        SignalValue::F64(value) => write!(writer, "{value}"),
    }
}

fn write_csv_cell(writer: &mut impl Write, value: &str) -> Result<(), String> {
    writer
        .write_all(escape_csv_cell(value).as_bytes())
        .map_err(|err| format!("failed writing CSV cell: {err}"))
}

fn escape_csv_cell(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{CsvTraceWriter, escape_csv_cell};
    use crate::sim::types::SignalValue;

    #[test]
    fn escape_csv_cell_quotes_commas_quotes_and_newlines() {
        assert_eq!(escape_csv_cell("simple"), "simple");
        assert_eq!(escape_csv_cell("a,b"), "\"a,b\"");
        assert_eq!(escape_csv_cell("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_csv_cell("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn create_escapes_header_cells() {
        let temp_dir = tempfile::tempdir().expect("tempdir should be creatable");
        let trace_path = temp_dir.path().join("trace.csv");
        let _writer = CsvTraceWriter::create(
            &trace_path,
            &[
                "signal,one".to_string(),
                "signal\"two".to_string(),
                "signal-three".to_string(),
            ],
        )
        .expect("trace writer should be creatable");

        let content = std::fs::read_to_string(trace_path).expect("trace header should be readable");
        let header = content.lines().next().expect("trace header should exist");
        assert_eq!(
            header,
            "tick,time_us,\"signal,one\",\"signal\"\"two\",signal-three"
        );
    }

    #[test]
    fn write_row_serializes_scalar_values_without_csv_escaping() {
        let temp_dir = tempfile::tempdir().expect("tempdir should be creatable");
        let trace_path = temp_dir.path().join("trace.csv");
        let mut writer =
            CsvTraceWriter::create(&trace_path, &["bool".to_string(), "num".to_string()])
                .expect("trace writer should be creatable");

        writer
            .write_row(7, 42, &[SignalValue::Bool(true), SignalValue::F32(1.5)])
            .expect("trace row should be writable");
        writer.flush().expect("trace writer should flush");

        let content = std::fs::read_to_string(trace_path).expect("trace file should be readable");
        assert_eq!(content, "tick,time_us,bool,num\n7,42,true,1.5\n");
    }
}
