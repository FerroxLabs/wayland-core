const MIN_FOLD_COUNT: usize = 3;
const MIN_PREFIX_RATIO: f64 = 0.5;

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(ca, cb)| ca == cb)
        .count()
}

fn lines_are_similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let prefix = common_prefix_len(a, b);
    // Normalise by the LONGER line. Dividing by the shorter one makes tiny
    // structural lines promiscuous: `  {` shares its two-space indent with
    // every field line beneath it, giving 2/3 = 0.67, so it anchors a fold
    // group and swallows a whole pretty-printed JSON object. That destroyed
    // every tool name in a ToolSearch result -- the one output that must stay
    // lossless, because it is the hydration path by which a deferred tool's
    // name reaches the model. The longer line is the honest denominator: a
    // 3-char line and a 30-char line are not similar in any useful sense.
    let span = a.len().max(b.len());
    prefix as f64 / span as f64 >= MIN_PREFIX_RATIO
}

pub fn fold_repeated_lines(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // `.lines()` is CRLF-tolerant — splits on `\n` and strips any trailing
    // `\r`, so files saved with Windows line endings fold identically to
    // their LF counterparts.
    let lines: Vec<&str> = text.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines_are_similar(lines[i], lines[j]) {
            j += 1;
        }

        let group_len = j - i;
        if group_len >= MIN_FOLD_COUNT {
            let folded = group_len - 2;
            result.push(lines[i].to_string());
            let identical = (i + 1..j).all(|k| lines[k] == lines[i]);
            if identical {
                result.push(format!("[... {folded} identical lines]"));
            } else {
                result.push(format!("[... {folded} similar lines]"));
            }
            result.push(lines[j - 1].to_string());
        } else {
            for line in &lines[i..j] {
                result.push(line.to_string());
            }
        }

        i = j;
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_identical_consecutive_lines() {
        let input = "ok\nok\nok\nok\nok\ndone";
        let result = fold_repeated_lines(input);
        assert!(result.contains("[... 3 identical lines]"));
        assert!(result.contains("ok"));
        assert!(result.contains("done"));
    }

    #[test]
    fn fold_no_repeats_unchanged() {
        let input = "apple\nbanana\ncherry";
        assert_eq!(fold_repeated_lines(input), input);
    }

    #[test]
    fn fold_similar_prefix_lines() {
        let lines: Vec<String> = (0..10)
            .map(|i| format!("Compiling crate-{i} v0.1.0"))
            .collect();
        let input = lines.join("\n");
        let result = fold_repeated_lines(&input);
        assert!(result.contains("[... 8 similar lines]"));
        assert!(result.contains("Compiling crate-0"));
        assert!(result.contains("Compiling crate-9"));
    }

    #[test]
    fn fold_below_threshold_unchanged() {
        let input = "Compiling a v0.1.0\nCompiling b v0.1.0\ndone";
        assert_eq!(fold_repeated_lines(input), input);
    }

    #[test]
    fn fold_mixed_groups() {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!("Downloading dep-{i}..."));
        }
        lines.push("Install complete".to_string());
        for i in 0..5 {
            lines.push(format!("Compiling mod-{i}"));
        }
        let input = lines.join("\n");
        let result = fold_repeated_lines(&input);
        assert!(
            result.contains("[... 4 similar lines]"),
            "first group folded: {result}"
        );
        assert!(result.contains("Install complete"));
        assert!(
            result.contains("[... 3 similar lines]"),
            "second group folded: {result}"
        );
    }

    /// The exact mechanism that destroyed every ToolSearch result: a group is
    /// anchored on `lines[i]`, so a 3-char structural line like `  {` was
    /// compared against every field line beneath it. Sharing only the 2-space
    /// indent gave 2/3 = 0.67 under the old shorter-line denominator, so the
    /// brace swallowed the whole object.
    ///
    /// MUTANT CHECK: revert `span` to `a.len().min(b.len())` and this fails
    /// with the body collapsed to a `[... N similar lines]` marker.
    #[test]
    fn a_brace_line_does_not_swallow_the_fields_beneath_it() {
        let input = "  {\n    \"name\": \"chart_get_state\",\n    \"kind\": \"query\",\n  },";
        let result = fold_repeated_lines(input);
        assert_eq!(
            result, input,
            "the `  {{` line anchored a fold group and ate the fields: {result}"
        );
    }

    /// Guards the direction of the fix. A short line and a long line are not
    /// similar merely because the short one is a prefix-ish of the long one.
    #[test]
    fn a_short_line_is_not_similar_to_a_much_longer_one() {
        assert!(!lines_are_similar(
            "  {",
            "    \"name\": \"chart_get_state\","
        ));
        assert!(!lines_are_similar(
            "]",
            "  \"description\": \"a long description\""
        ));
    }

    /// NEGATIVE CONTROL for the two above. Without this, the fix could be
    /// "never fold anything" and every other test here would still pass.
    #[test]
    fn equal_length_repetitive_lines_are_still_similar() {
        assert!(lines_are_similar(
            "Compiling crate-1 v0.1.0",
            "Compiling crate-2 v0.1.0"
        ));
        assert!(lines_are_similar("ok", "ok"));
    }

    #[test]
    fn fold_empty_input() {
        assert_eq!(fold_repeated_lines(""), "");
    }
}
