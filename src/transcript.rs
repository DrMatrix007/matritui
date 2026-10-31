use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptStats {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub message_count: usize,
}

pub fn encode_project_dir(cwd: &Path) -> String {
    cwd.to_string_lossy().replace(['/', '.'], "-")
}

pub fn transcripts_root() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".claude").join("projects"))
        .unwrap_or_else(|| PathBuf::from(".claude").join("projects"))
}

pub fn transcript_path(transcripts_root: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    transcripts_root.join(encode_project_dir(cwd)).join(format!("{session_id}.jsonl"))
}

pub fn parse_transcript(path: &Path) -> io::Result<TranscriptStats> {
    let file = std::fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut stats = TranscriptStats::default();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };

        match value.get("type").and_then(|v| v.as_str()) {
            Some("assistant") => {
                let usage = value.get("message").and_then(|m| m.get("usage"));
                let input_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0);
                let output_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(|v| v.as_u64()).unwrap_or(0);
                stats.total_input_tokens += input_tokens;
                stats.total_output_tokens += output_tokens;
                stats.message_count += 1;
            }
            Some("cost-state") => {
                if let Some(total_cost_usd) = value.get("totalCostUSD").and_then(|v| v.as_f64()) {
                    stats.total_cost_usd = total_cost_usd;
                }
            }
            _ => {}
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mmux-test-transcript-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn parse_transcript_sums_fields_across_multiple_lines() {
        let path = unique_temp_path("sums-fields");
        let contents = concat!(
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"usage":{"input_tokens":5,"output_tokens":7}}}"#,
            "\n",
            r#"{"type":"cost-state","totalCostUSD":1.25}"#,
            "\n",
            r#"{"type":"cost-state","totalCostUSD":2.5}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
            "\n",
        );
        std::fs::write(&path, contents).expect("write fixture transcript");

        let stats = parse_transcript(&path).expect("parse transcript");

        assert_eq!(stats.total_input_tokens, 15);
        assert_eq!(stats.total_output_tokens, 27);
        assert_eq!(stats.message_count, 2);
        assert_eq!(stats.total_cost_usd, 2.5);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parse_transcript_skips_truncated_trailing_line_without_erroring() {
        let path = unique_temp_path("truncated-trailing-line");
        let contents = concat!(
            r#"{"type":"assistant","message":{"usage":{"input_tokens":3,"output_tokens":4}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"usage":{"input_"#,
        );
        std::fs::write(&path, contents).expect("write fixture transcript");

        let stats = parse_transcript(&path).expect("parse transcript should not error on truncated line");

        assert_eq!(stats.total_input_tokens, 3);
        assert_eq!(stats.total_output_tokens, 4);
        assert_eq!(stats.message_count, 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parse_transcript_on_missing_file_returns_err_not_panic() {
        let path = unique_temp_path("does-not-exist");

        let result = parse_transcript(&path);

        assert!(result.is_err());
    }

    #[test]
    fn encode_project_dir_replaces_slashes_and_dots_with_dashes() {
        assert_eq!(
            encode_project_dir(Path::new("/home/ofrih/Projects/matritui")),
            "-home-ofrih-Projects-matritui"
        );
        assert_eq!(encode_project_dir(Path::new("/home/ofrih/.claude")), "-home-ofrih--claude");
    }

    #[test]
    fn transcript_path_joins_root_encoded_cwd_and_session_file() {
        let root = PathBuf::from("/root");
        let cwd = Path::new("/home/ofrih/.claude");
        let path = transcript_path(&root, cwd, "sess-123");

        assert_eq!(path, root.join("-home-ofrih--claude").join("sess-123.jsonl"));
    }
}
