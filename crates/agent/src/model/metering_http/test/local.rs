use super::*;

fn chat_request() -> Request<Bytes> {
    Request::builder()
        .uri("http://host.docker.internal:11434/v1/chat/completions")
        .body(Bytes::from_static(
            br#"{"model":"qwen3:30b","messages":[],"max_tokens":50}"#,
        ))
        .unwrap()
}

fn chat_response() -> Bytes {
    Bytes::from_static(
        br#"{"id":"c-1","choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
    )
}

#[tokio::test]
async fn a_model_the_journal_has_no_rate_for_is_refused_when_metered() {
    let journal = Arc::new(Journal::default());
    let transport = Transport::new(chat_response());
    let client = MeteredHttpClient::new(transport.clone(), "local", WireProtocol::ChatCompletions);
    context(
        journal.clone(),
        WireProtocol::ChatCompletions,
        "openai",
        "gpt-5.5",
    )
    .scope(async {
        assert!(client.send::<_, Bytes>(chat_request()).await.is_err());
    })
    .await;

    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unmetered_client_sends_the_request_untouched_and_journals_nothing() {
    let journal = Arc::new(Journal::default());
    let transport = Transport::new(chat_response());
    let client = MeteredHttpClient::new(transport.clone(), "local", WireProtocol::ChatCompletions)
        .unmetered();
    context(
        journal.clone(),
        WireProtocol::ChatCompletions,
        "openai",
        "gpt-5.5",
    )
    .scope(async {
        client.send::<_, Bytes>(chat_request()).await.unwrap();
    })
    .await;

    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    // The body is exactly what the model server was asked for: no token budget
    // constraint was applied.
    assert_eq!(transport.bodies.lock().unwrap()[0]["max_tokens"], 50);
    assert!(journal.begins.lock().unwrap().is_empty());
    assert!(journal.finals.lock().unwrap().is_empty());
}
