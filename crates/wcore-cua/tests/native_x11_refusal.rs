//! Separate safety proof. The original native background-input RED is retained
//! in its original candidate; this test does not turn refusal into capability.
#![cfg(all(target_os = "linux", feature = "x11"))]

use base64::Engine as _;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use wcore_cua::{
    backend::{CuaSession, Region, ScreenshotFormat},
    backends::linux_x11::LinuxX11Backend,
    error::CuaError,
    op::{CuaOp, CuaOpResult},
    policy::CuaPolicy,
    tool::CuaTool,
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
            WindowClass,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

struct OwnedWindows {
    connection: RustConnection,
    front: u32,
    target: u32,
}
impl Drop for OwnedWindows {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.front);
        let _ = self.connection.destroy_window(self.target);
        let _ = self.connection.flush();
    }
}
fn atom(connection: &RustConnection, name: &[u8]) -> u32 {
    connection
        .intern_atom(false, name)
        .unwrap()
        .reply()
        .unwrap()
        .atom
}
fn drain(connection: &RustConnection) -> Vec<Event> {
    let mut events = vec![];
    while let Some(event) = connection.poll_for_event().unwrap() {
        events.push(event);
    }
    events
}
fn input(event: &Event) -> bool {
    matches!(
        event,
        Event::ButtonPress(_) | Event::ButtonRelease(_) | Event::KeyPress(_) | Event::KeyRelease(_)
    )
}
fn state(connection: &RustConnection, root: u32, stack: u32) -> (u32, i16, i16, Vec<u32>) {
    let focus = connection.get_input_focus().unwrap().reply().unwrap().focus;
    let pointer = connection.query_pointer(root).unwrap().reply().unwrap();
    let stacking = connection
        .get_property(false, root, stack, AtomEnum::WINDOW, 0, 1024)
        .unwrap()
        .reply()
        .unwrap()
        .value32()
        .unwrap()
        .collect();
    (focus, pointer.root_x, pointer.root_y, stacking)
}
async fn settled(connection: &RustConnection) {
    connection.flush().unwrap();
    connection.get_input_focus().unwrap().reply().unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
}

#[tokio::test]
#[ignore = "requires the owned Xvfb+WM wrapper; explicit native acceptance only"]
async fn x11_refuses_all_input_without_events_or_operator_state_changes() {
    assert_eq!(std::env::var("WCORE_CUA_NATIVE_X11").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("DISPLAY").unwrap(),
        std::env::var("WCORE_CUA_OWNED_DISPLAY").unwrap()
    );
    let proof = PathBuf::from(std::env::var_os("WCORE_CUA_PROOF_DIR").unwrap());
    let (connection, screen) = x11rb::connect(None).unwrap();
    let screen = connection.setup().roots[screen].clone();
    let front = connection.generate_id().unwrap();
    let target = connection.generate_id().unwrap();
    let owned = OwnedWindows {
        connection,
        front,
        target,
    };
    let connection = &owned.connection;
    let wm = atom(connection, b"_NET_SUPPORTING_WM_CHECK");
    assert!(
        connection
            .get_property(false, screen.root, wm, AtomEnum::WINDOW, 0, 1)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .and_then(|mut values| values.next())
            .is_some(),
        "representative WM required"
    );
    for (window, x, color) in [(front, 30, 0x2244cc), (target, 400, 0x33cc55)] {
        connection
            .create_window(
                screen.root_depth,
                window,
                screen.root,
                x,
                80,
                180,
                120,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new().background_pixel(color).event_mask(
                    EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE,
                ),
            )
            .unwrap()
            .check()
            .unwrap();
        let class = if window == front {
            b"front\0W15RefusalFront\0".as_slice()
        } else {
            b"target\0W15RefusalTarget\0".as_slice()
        };
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                class,
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
    }
    settled(connection).await;
    let active = atom(connection, b"_NET_ACTIVE_WINDOW");
    connection
        .send_event(
            false,
            screen.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, front, active, [2, x11rb::CURRENT_TIME, 0, 0, 0]),
        )
        .unwrap()
        .check()
        .unwrap();
    settled(connection).await;
    let point = connection
        .translate_coordinates(front, screen.root, 50, 50)
        .unwrap()
        .reply()
        .unwrap();
    connection
        .warp_pointer(
            x11rb::NONE,
            screen.root,
            0,
            0,
            0,
            0,
            point.dst_x,
            point.dst_y,
        )
        .unwrap()
        .check()
        .unwrap();
    settled(connection).await;
    drain(connection);
    // The independent X11 event observer must first observe a real native
    // fixture-controlled press; this is not a backend success claim.
    connection
        .xtest_fake_input(4, 1, 0, screen.root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    connection
        .xtest_fake_input(5, 1, 0, screen.root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    settled(connection).await;
    assert!(
        drain(connection)
            .iter()
            .any(|event| matches!(event, Event::ButtonPress(e) if e.event == front)),
        "event observer positive control"
    );
    let stack = atom(connection, b"_NET_CLIENT_LIST_STACKING");
    let before = state(connection, screen.root, stack);
    assert_eq!(before.0, front);
    assert!(before.3.contains(&front) && before.3.contains(&target));
    let tool = CuaTool::new(Arc::new(LinuxX11Backend::new()), CuaPolicy::permissive());
    let screenshot = tool
        .dispatch(
            CuaSession::for_test("refusal-safe-capture"),
            CuaOp::Screenshot {
                region: Region::Full,
                format: ScreenshotFormat::Png,
                redact: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let CuaOpResult::Screenshot {
        data_b64,
        width,
        height,
        ..
    } = screenshot
    else {
        panic!("actual screenshot required")
    };
    assert_eq!(
        (width, height),
        (
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels)
        )
    );
    let png = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(
        image.get_pixel(point.dst_x as u32, point.dst_y as u32).0,
        [0x22, 0x44, 0xcc, 0xff]
    );
    std::fs::write(proof.join("x11-refusal-screenshot.png"), png).unwrap();
    assert!(matches!(
        tool.dispatch(
            CuaSession::for_test("wait"),
            CuaOp::Wait { duration_ms: 1 },
            CancellationToken::new()
        )
        .await,
        Ok(CuaOpResult::Ok)
    ));
    assert!(
        matches!(tool.dispatch(CuaSession::for_test("frontmost"), CuaOp::FrontmostApp {}, CancellationToken::new()).await, Ok(CuaOpResult::FrontmostApp { app_id: Some(ref app) }) if app == "W15RefusalFront")
    );
    let mut refused = vec![];
    for op in CuaOp::all_variants_for_test().into_iter().filter(|op| {
        matches!(
            op,
            CuaOp::LeftClick { .. }
                | CuaOp::RightClick { .. }
                | CuaOp::DoubleClick { .. }
                | CuaOp::MouseMove { .. }
                | CuaOp::Scroll { .. }
                | CuaOp::Type { .. }
                | CuaOp::Key { .. }
        )
    }) {
        let kind = op.kind_tag();
        let result = tool
            .dispatch(
                CuaSession::for_test("native-refusal"),
                op,
                CancellationToken::new(),
            )
            .await;
        assert!(
            matches!(result, Err(CuaError::UnsupportedPlatform(reason)) if reason.contains("background input")),
            "{kind} must refuse"
        );
        settled(connection).await;
        assert!(
            !drain(connection).iter().any(input),
            "{kind} emitted native input despite refusal"
        );
        assert_eq!(
            state(connection, screen.root, stack),
            before,
            "{kind} changed pointer/focus/stack"
        );
        refused.push(kind);
    }
    assert_eq!(refused.len(), 7);
    let receipt = serde_json::json!({"platform":"linux-x11","safety_containment_pass":true,"background_input_capability":"unresolved",
        "screenshot_positive":true,"wait_positive":true,"frontmost_query_positive":true,"native_event_observer_control":true,
        "refused_ops":refused,"operator_state_before":before,"operator_state_after":state(connection, screen.root, stack)});
    std::fs::write(
        proof.join("x11-refusal.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("{receipt}");
}
