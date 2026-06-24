use std::time::Duration;

use async_nats::jetstream::{self, consumer::pull, stream};
use futures_util::StreamExt;

pub type JetStream = jetstream::Context;

/// Connect to NATS and ensure the MESSAGES stream exists.
/// Returns both the raw Client (for flush on shutdown) and the JetStream context.
pub async fn connect_and_setup(url: &str) -> (async_nats::Client, JetStream) {
    let client = async_nats::connect(url)
        .await
        .unwrap_or_else(|e| panic!("NATS connect failed at {url}: {e}"));

    let js = jetstream::new(client.clone());

    js.get_or_create_stream(stream::Config {
        name: "MESSAGES".into(),
        subjects: vec!["msg.>".into()],
        max_age: Duration::from_secs(7 * 24 * 3600),
        max_messages_per_subject: 10_000,
        max_bytes: 1_073_741_824, // 1 GB
        ..Default::default()
    })
    .await
    .unwrap_or_else(|e| panic!("MESSAGES stream setup failed: {e}"));

    (client, js)
}

pub async fn publish(js: &JetStream, user_id: &str, payload: Vec<u8>) {
    let subject = format!("msg.{user_id}");
    if let Err(e) = js.publish(subject, payload.into()).await {
        tracing::error!(err = %e, user_id, "nats publish offline message failed");
    }
}

pub async fn drain_pending(js: &JetStream, user_id: &str) -> Vec<Vec<u8>> {
    let stream = match js.get_stream("MESSAGES").await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(err = %e, "failed to get MESSAGES stream");
            return Vec::new();
        }
    };

    let mut consumer = match stream
        .create_consumer(pull::Config {
            filter_subject: format!("msg.{user_id}"),
            ..Default::default()
        })
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(err = %e, user_id, "failed to create pull consumer");
            return Vec::new();
        }
    };

    let pending = match consumer.info().await {
        Ok(info) => info.num_pending as usize,
        Err(e) => {
            tracing::error!(err = %e, user_id, "failed to get consumer info");
            return Vec::new();
        }
    };

    if pending == 0 {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(pending);
    let mut remaining = pending;

    while remaining > 0 {
        let batch_size = remaining.min(256);
        let batch = match consumer
            .fetch()
            .max_messages(batch_size)
            .expires(Duration::from_secs(1))
            .messages()
            .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(err = %e, user_id, "nats fetch failed");
                break;
            }
        };

        let mut pinned = std::pin::pin!(batch);
        let mut got = 0;
        while let Some(msg_result) = pinned.next().await {
            match msg_result {
                Ok(msg) => {
                    result.push(msg.payload.to_vec());
                    let _ = msg.ack().await;
                    got += 1;
                }
                Err(e) => {
                    tracing::warn!(err = %e, "nats message error during drain");
                    break;
                }
            }
        }

        if got == 0 {
            break;
        }
        remaining = remaining.saturating_sub(got);
    }

    tracing::debug!(user_id, count = result.len(), "drained pending offline messages");
    result
}
