//! AppleScript helpers — building + executing osascript commands.
//!
//! All user-controlled values must go through [`quote_applescript_string`]
//! before interpolation. Never pass raw user input into the script body.

use crate::error::IMessageError;

/// AppleScript-quote a string value. Escapes backslashes and double-quotes,
/// then wraps in double-quotes. This is the minimal safe quoting for
/// AppleScript string literals.
pub fn quote_applescript_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Run an osascript expression and return stdout on success.
/// Returns `IMessageError::AutomationDenied` when stderr indicates TCC denial.
/// Returns `IMessageError::ChatNotFound` when stderr indicates -1728 (cache miss).
pub async fn run_osascript(script: &str, timeout_ms: u64) -> Result<String, IMessageError> {
    use tokio::process::Command;
    use tokio::time::{Duration, timeout};

    let fut = async {
        Command::new("osascript")
            .args(["-e", script])
            .output()
            .await
    };

    let output = timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .map_err(|_| IMessageError::AppleScript {
            exit_code: -1,
            stderr: "osascript timed out".to_string(),
        })?
        .map_err(|e| IMessageError::AppleScript {
            exit_code: -1,
            stderr: e.to_string(),
        })?;

    let exit_code = output.status.code().unwrap_or(-1);
    if exit_code != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_automation_denied(&stderr) {
            return Err(IMessageError::AutomationDenied);
        }
        if is_chat_not_found(&stderr) {
            return Err(IMessageError::ChatNotFound);
        }
        return Err(IMessageError::AppleScript { exit_code, stderr });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn is_automation_denied(stderr: &str) -> bool {
    stderr.contains("not allowed to send Apple events")
        || stderr.contains("-1743")
        || stderr.contains("AppleScript")
}

fn is_chat_not_found(stderr: &str) -> bool {
    stderr.contains("-1728") || stderr.contains("Can't get chat id")
}

/// Build an osascript to send a plain-text iMessage.
///
/// - Group chats: chatId is a GUID like `chat<hex>` — addressed by `chat id`.
/// - 1:1 chats: chatId is a phone/email — addressed by `buddy`.
///   `service_name` ("iMessage" / "SMS") picks the service so green-bubble
///   recipients don't silently queue on the wrong service.
pub fn build_send_script(chat_id: &str, text: &str, service_name: Option<&str>) -> String {
    let quoted_text = quote_applescript_string(text);

    // Group chat GUID: starts with "chat" followed by hex digits.
    if chat_id.to_ascii_lowercase().starts_with("chat")
        && chat_id[4..].chars().all(|c| c.is_ascii_hexdigit())
    {
        let quoted_guid = quote_applescript_string(chat_id);
        return format!(
            "tell application \"Messages\"\n  \
               set targetChat to chat id {quoted_guid}\n  \
               send {quoted_text} to targetChat\n\
             end tell"
        );
    }

    // 1:1 handle.
    let quoted_handle = quote_applescript_string(chat_id);
    let use_sms = service_name
        .map(|s| s.to_uppercase() == "SMS")
        .unwrap_or(false);
    let service_type = if use_sms { "SMS" } else { "iMessage" };
    format!(
        "tell application \"Messages\"\n  \
           set targetService to 1st service whose service type = {service_type}\n  \
           set targetBuddy to buddy {quoted_handle} of targetService\n  \
           send {quoted_text} to targetBuddy\n\
         end tell"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes_double_quote_and_backslash() {
        assert_eq!(
            quote_applescript_string(r#"say "hello""#),
            r#""say \"hello\"""#
        );
        assert_eq!(
            quote_applescript_string(r"path\to\file"),
            r#""path\\to\\file""#
        );
    }

    #[test]
    fn build_send_script_group_chat() {
        let script = build_send_script("chatdeadbeef", "hello", None);
        assert!(script.contains("chat id"), "should use chat id idiom");
        assert!(script.contains("\"chatdeadbeef\""));
        assert!(script.contains("\"hello\""));
    }

    #[test]
    fn build_send_script_one_to_one_imessage() {
        let script = build_send_script("+15551234567", "hi", Some("iMessage"));
        assert!(script.contains("service type = iMessage"));
        assert!(script.contains("buddy"));
    }

    #[test]
    fn build_send_script_one_to_one_sms() {
        let script = build_send_script("+15551234567", "hi", Some("SMS"));
        assert!(script.contains("service type = SMS"));
    }
}
