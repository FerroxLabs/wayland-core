p = "crates/wcore-channel-discord/src/lib.rs"
s = open(p).read()

old = """        ch.send_message_idempotent(msg, key).await.unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// Wrong-refusal twin:"""
new = """        let result = ch.send_message_idempotent(msg, key).await;

        // Asserted on the MOCK, not on the send: a body carrying an
        // unexpected `message_reference` simply matches no mock, and the
        // `Transport("server 501")` that comes back would name the symptom
        // rather than the defect.
        assert!(
            mock.matched_async().await,
            "Discord put something other than {{content, nonce}} on the wire: \\
             the thread DESTINATION was spent as a message_reference \\
             (FerroxLabs/wayland-core#363 c6). Send result: {result:?}"
        );
        result.expect("the send itself must still succeed");
        ch.stop().await.unwrap();
    }

    /// Wrong-refusal twin:"""
assert old in s, "arm1 anchor miss"
s = s.replace(old, new, 1)

old2 = """        ch.send_message_idempotent(msg, key).await.unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }
}"""
new2 = """        let result = ch.send_message_idempotent(msg, key).await;

        assert!(
            mock.matched_async().await,
            "Discord dropped or altered a GENUINE reply's message_reference \\
             (FerroxLabs/wayland-core#363 c6 control). Send result: {result:?}"
        );
        result.expect("the send itself must still succeed");
        ch.stop().await.unwrap();
    }
}"""
assert old2 in s, "arm2 anchor miss"
s = s.replace(old2, new2, 1)
open(p, "w").write(s)
print("sharpened")
