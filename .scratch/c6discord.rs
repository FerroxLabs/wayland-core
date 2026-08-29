
    // -----------------------------------------------------------------
    // FerroxLabs/wayland-core#363 c6 — a thread DESTINATION must never be
    // spent as a quoted message.
    //
    // `channel_send_transport` used to fall back to putting the target's
    // third segment (`platform:chat:thread`) into `OutgoingMessage::reply_to`.
    // Telegram then forwarded it as `reply_to_message_id`, which is the defect
    // #363 is filed about; Discord reads `reply_to` as a genuine
    // `message_reference` and routes threads by `conversation_id` instead, so
    // the SAME fallback put a thread CHANNEL id into a message reference. Best
    // case the API refuses; worse, the snowflake collides with a real
    // unrelated message and the bot quotes a stranger.
    //
    // The fallback is gone, but nothing here graded the connector: the only
    // arm was `a_reply_inherits_the_thread_as_a_destination_and_the_quote_
    // separately` in `wcore-agent`, which grades the shared transport and
    // names no connector. Inferring a connector's behaviour from a shared
    // helper is exactly how the Telegram defect survived in the first place,
    // so this drives Discord's own send path to its own wire body.
    //
    // Both arms use `send_message_idempotent` so the nonce is derived and the
    // body can be matched EXACTLY. Exactness is the point: `message_reference`
    // is `skip_serializing_if = "Option::is_none"`, so its ABSENCE is only
    // observable against a whole-body match, and a partial match would pass on
    // a body that carried it.
    // -----------------------------------------------------------------

    /// A thread destination alone must leave `message_reference` off the wire.
    #[tokio::test]
    async fn discord_never_sends_a_thread_destination_as_a_message_reference() {
        let mut server = mockito::Server::new_async().await;
        let key = "cron:thread-dest:1";
        let nonce = rest::nonce_for_key(key);
        let mock = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "content": "hello",
                "nonce": nonce,
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"42","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let msg = OutgoingMessage {
            // A Discord thread id is a channel snowflake, indistinguishable in
            // shape from a message snowflake -- which is why the old fallback
            // produced a well-formed reference to the wrong object rather than
            // an obvious error.
            thread_id: Some("1276543210987654321".to_string()),
            ..OutgoingMessage::text(TEST_CHANNEL, "hello")
        };
        ch.send_message_idempotent(msg, key).await.unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// Wrong-refusal twin: a GENUINE quoted message must still become a
    /// `message_reference`. Without this, "no message_reference" is satisfied
    /// by a connector that stopped sending replies at all.
    #[tokio::test]
    async fn discord_still_quotes_a_genuine_reply_to_message() {
        let mut server = mockito::Server::new_async().await;
        let key = "cron:thread-dest:2";
        let nonce = rest::nonce_for_key(key);
        let mock = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "content": "hello",
                "nonce": nonce,
                "message_reference": { "message_id": "999888777666555444" },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"43","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let msg = OutgoingMessage {
            reply_to: Some("999888777666555444".to_string()),
            // Both set at once: the quote is the one that reaches the wire and
            // the destination must not overwrite or duplicate it.
            thread_id: Some("1276543210987654321".to_string()),
            ..OutgoingMessage::text(TEST_CHANNEL, "hello")
        };
        ch.send_message_idempotent(msg, key).await.unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }
}
