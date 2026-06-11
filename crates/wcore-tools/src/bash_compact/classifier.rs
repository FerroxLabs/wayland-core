//! Generic output-shape fallback compactor — used when no per-command parser
//! confidently handled the output. Classifies the output SHAPE (not the
//! command) and keeps the signal: error/warn/fail lines + counts, else a
//! head+tail with an omission marker. Returns `None` only if it would not
//! shrink the output.

/// Lines kept from the head and tail in the `raw` (unclassifiable) case.
const HEAD_LINES: usize = 15;
const TAIL_LINES: usize = 5;

pub(super) fn compact(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw.lines().collect();

    // Error/warning/failure-bearing output: keep only those lines + a count.
    let signal: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| {
            let t = l.trim_start();
            t.contains("error")
                || t.contains("Error")
                || t.contains("ERROR")
                || t.contains("warning")
                || t.contains("FAIL")
                || t.contains("failed")
                || t.starts_with('✗')
                || t.starts_with('×')
        })
        .collect();

    if !signal.is_empty() && signal.len() < lines.len() {
        let kept: Vec<&str> = signal.iter().copied().take(40).collect();
        return Some(format!(
            "[compacted: {} of {} lines — error/warn/fail only]\n{}",
            kept.len(),
            lines.len(),
            kept.join("\n")
        ));
    }

    // Otherwise head + tail with an omission marker.
    if lines.len() > HEAD_LINES + TAIL_LINES + 2 {
        let head = lines[..HEAD_LINES].join("\n");
        let tail = lines[lines.len() - TAIL_LINES..].join("\n");
        let omitted = lines.len() - HEAD_LINES - TAIL_LINES;
        return Some(format!("{head}\n... ({omitted} lines omitted) ...\n{tail}"));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_error_lines_when_present() {
        let mut raw = String::new();
        for i in 0..100 {
            raw.push_str(&format!("compiling unit {i}\n"));
        }
        raw.push_str("error: boom happened\n");
        let out = compact(&raw).expect("should compact");
        assert!(out.contains("error: boom happened"));
        assert!(!out.contains("compiling unit 50"));
        assert!(out.len() < raw.len());
    }

    #[test]
    fn head_tail_when_no_signal() {
        let raw = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = compact(&raw).expect("should compact");
        assert!(out.contains("line 0"));
        assert!(out.contains("line 99"));
        assert!(out.contains("lines omitted"));
        assert!(out.len() < raw.len());
    }

    #[test]
    fn returns_none_for_already_small() {
        assert!(compact("a\nb\nc").is_none());
    }
}
