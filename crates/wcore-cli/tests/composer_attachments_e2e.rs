//! Packaged JSON-stream proof for desktop composer image attachments.

#[path = "support/mod.rs"]
mod support;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::mock_llm::{MockLlm, received_requests};
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

const CONFIGURED_KEY: &str = "composer-fixture-credential";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn png_bytes(tail: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\ncomposer-png".to_vec();
    bytes.push(tail);
    bytes
}

fn jpeg_bytes(tail: u8) -> Vec<u8> {
    let mut bytes = b"\xff\xd8\xff\xe0composer-jpeg".to_vec();
    bytes.push(tail);
    bytes
}

fn write_config(home: &Path, base_url: &str) {
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
             \n[providers.anthropic]\napi_key = \"{CONFIGURED_KEY}\"\nbase_url = \"{base_url}\"\n\
             \n[session]\nenabled = false\n"
        ),
    )
    .expect("write config");
}

fn wait_for_stream_end(frames: &Arc<Mutex<Vec<Value>>>, msg_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| frame["type"] == "stream_end" && frame["msg_id"].as_str() == Some(msg_id))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "timed out waiting for stream_end({msg_id}); frames={:?}",
        frames.lock().unwrap()
    );
}

fn send_message(
    stdin: &mut impl Write,
    frames: &Arc<Mutex<Vec<Value>>>,
    msg_id: &str,
    content: &str,
    files: Vec<String>,
) {
    let wire = json!({
        "type": "message",
        "msg_id": msg_id,
        "content": content,
        "files": files,
    });
    writeln!(stdin, "{wire}").expect("write exact serialized message/files command");
    stdin.flush().expect("flush command");
    wait_for_stream_end(frames, msg_id);
}

#[test]
fn packaged_json_stream_ingests_images_on_active_provider_and_rejects_bad_files() {
    let home = TempDir::new().expect("home tempdir");
    let fixture_dir = TempDir::new().expect("fixture tempdir");
    let png = fixture_dir.path().join("first.png");
    let jpeg = fixture_dir.path().join("second.jpeg");
    std::fs::write(&png, png_bytes(1)).expect("write PNG");
    std::fs::write(&jpeg, jpeg_bytes(2)).expect("write JPEG");

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = runtime.block_on(
        MockLlm::new()
            .text("text plus images")
            .text("image only")
            .start(),
    );
    write_config(home.path(), &server.uri());

    let mut command = std::process::Command::new(binary());
    command
        .args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path())
        .env("WAYLAND_HOME", home.path())
        .env("HOME", home.path())
        .env("TERM", "dumb")
        .env_remove("API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let vault = support::vault::configure_process(&mut command);
    let child = command.spawn();
    drop(vault);
    let mut child = child.expect("spawn packaged JSON-stream binary");
    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let frames = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let frames = Arc::clone(&frames);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(frame) = serde_json::from_str(&line) {
                    frames.lock().unwrap().push(frame);
                }
            }
        });
    }

    let missing = fixture_dir.path().join("missing.png");
    let directory = fixture_dir.path().to_path_buf();
    let oversized = fixture_dir.path().join("oversized.png");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(20 * 1024 * 1024 + 1)
        .unwrap();
    let mismatch = fixture_dir.path().join("mismatch.jpg");
    std::fs::write(&mismatch, png_bytes(3)).unwrap();
    let corrupt = fixture_dir.path().join("corrupt.png");
    std::fs::write(&corrupt, b"not a real image payload").unwrap();
    let traversal = fixture_dir
        .path()
        .join("child")
        .join("..")
        .join("first.png");

    let mut invalid = vec![
        ("missing", missing.to_string_lossy().into_owned()),
        ("directory", directory.to_string_lossy().into_owned()),
        ("oversized", oversized.to_string_lossy().into_owned()),
        ("mismatch", mismatch.to_string_lossy().into_owned()),
        ("corrupt", corrupt.to_string_lossy().into_owned()),
        ("traversal", traversal.to_string_lossy().into_owned()),
        ("remote", "https://example.com/image.png".into()),
        ("network-file-url", "file://server/share/image.png".into()),
        ("unc", r"\\server\share\image.png".into()),
    ];

    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::symlink;

        let symlink_path = fixture_dir.path().join("symlink.png");
        symlink(&png, &symlink_path).unwrap();
        invalid.push(("symlink", symlink_path.to_string_lossy().into_owned()));

        let fifo = fixture_dir.path().join("fifo.png");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the CString is a valid pathname for this isolated fixture.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        invalid.push(("fifo", fifo.to_string_lossy().into_owned()));
    }

    for (msg_id, path) in invalid {
        send_message(&mut stdin, &frames, msg_id, "reject this", vec![path]);
        assert!(
            frames.lock().unwrap().iter().any(|frame| {
                frame["type"] == "error"
                    && frame["msg_id"].as_str() == Some(msg_id)
                    && frame["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("composer attachment rejected"))
            }),
            "attachment rejection must be a valid error frame correlated to {msg_id}: {:?}",
            frames.lock().unwrap()
        );
        let requests = runtime.block_on(received_requests(&server));
        assert!(
            requests.is_empty(),
            "invalid attachment {msg_id} must fail before provider dispatch"
        );
    }

    send_message(
        &mut stdin,
        &frames,
        "text-images",
        "describe in order",
        vec![
            png.to_string_lossy().into_owned(),
            jpeg.to_string_lossy().into_owned(),
        ],
    );
    send_message(
        &mut stdin,
        &frames,
        "image-only",
        "",
        vec![png.to_string_lossy().into_owned()],
    );

    let requests = runtime.block_on(received_requests(&server));
    assert_eq!(requests.len(), 2, "only the two valid turns may dispatch");
    assert!(
        requests
            .iter()
            .all(|request| request.api_key.as_deref() == Some(CONFIGURED_KEY)),
        "both image turns must use the active session provider credential"
    );

    let first = requests[0].body["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"]
        .as_array()
        .unwrap();
    assert_eq!(
        first[0],
        json!({"type": "text", "text": "describe in order"})
    );
    assert_eq!(first[1]["type"], "image");
    assert_eq!(first[1]["source"]["media_type"], "image/png");
    assert_eq!(
        STANDARD
            .decode(first[1]["source"]["data"].as_str().unwrap())
            .unwrap(),
        png_bytes(1)
    );
    assert_eq!(first[2]["type"], "image");
    assert_eq!(first[2]["source"]["media_type"], "image/jpeg");
    assert_eq!(
        STANDARD
            .decode(first[2]["source"]["data"].as_str().unwrap())
            .unwrap(),
        jpeg_bytes(2)
    );

    let second = requests[1].body["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"]
        .as_array()
        .unwrap();
    assert!(
        second
            .iter()
            .filter(|part| part["type"] == "text")
            .all(|part| {
                part["text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
            }),
        "image-only turn must not inject an empty text block: {second:?}"
    );
    let images: Vec<_> = second
        .iter()
        .filter(|part| part["type"] == "image")
        .collect();
    assert_eq!(images.len(), 1, "image-only turn must retain its one image");
    assert_eq!(
        STANDARD
            .decode(images[0]["source"]["data"].as_str().unwrap())
            .unwrap(),
        png_bytes(1)
    );

    assert!(
        !frames.lock().unwrap().iter().any(|frame| {
            frame["type"] == "error"
                && matches!(frame["msg_id"].as_str(), Some("text-images" | "image-only"))
        }),
        "valid composer images must not route through an unconfigured auxiliary vision backend"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn packaged_json_stream_text_only_provider_degrades_with_actionable_placeholder() {
    let home = TempDir::new().expect("home tempdir");
    let fixture_dir = TempDir::new().expect("fixture tempdir");
    let png = fixture_dir.path().join("image.png");
    std::fs::write(&png, png_bytes(9)).expect("write PNG");

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = runtime.block_on(MockServer::start());
    let response = concat!(
        "data: {\"id\":\"text-only\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"degraded safely\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"text-only\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"text-only\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek-chat\",\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3,\"total_tokens\":10}}\n\n",
        "data: [DONE]\n\n"
    );
    runtime.block_on(async {
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(response, "text/event-stream"),
            )
            .mount(&server)
            .await;
    });
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[default]\nprovider = \"deepseek\"\nmodel = \"deepseek-chat\"\n\
             \n[providers.deepseek]\napi_key = \"text-only-fixture-key\"\nbase_url = \"{}\"\n\
             \n[session]\nenabled = false\n",
            server.uri()
        ),
    )
    .expect("write config");

    let mut command = std::process::Command::new(binary());
    command
        .args(["--json-stream", "--provider", "deepseek"])
        .current_dir(home.path())
        .env("WAYLAND_HOME", home.path())
        .env("HOME", home.path())
        .env("TERM", "dumb")
        .env_remove("DEEPSEEK_API_KEY")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let vault = support::vault::configure_process(&mut command);
    let child = command.spawn();
    drop(vault);
    let mut child = child.expect("spawn text-only packaged binary");
    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let frames = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let frames = Arc::clone(&frames);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(frame) = serde_json::from_str(&line) {
                    frames.lock().unwrap().push(frame);
                }
            }
        });
    }
    send_message(
        &mut stdin,
        &frames,
        "text-only",
        "describe",
        vec![png.to_string_lossy().into_owned()],
    );

    let requests = runtime.block_on(server.received_requests()).unwrap();
    assert_eq!(
        requests.len(),
        1,
        "text-only degradation must still dispatch"
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let user = body["messages"].as_array().unwrap().last().unwrap();
    let content = user["content"].as_str().expect("text-only user content");
    assert!(
        content.starts_with("describe\n")
            && content.ends_with("[image omitted: model not vision-capable]"),
        "degradation must preserve the prompt and explicitly describe the omitted image: {content:?}"
    );
    assert!(
        !user.to_string().contains("image_url"),
        "text-only provider must not receive the image part that would cause a 400"
    );
    assert!(
        frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| { frame["type"] == "stream_end" && frame["msg_id"] == "text-only" }),
        "the degraded request must complete with valid protocol framing"
    );

    let _ = child.kill();
    let _ = child.wait();
}
