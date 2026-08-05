//! Bounded HTTP body and streaming codecs for provider adapters.

use crate::diagnostics::{
    content_type_diagnostic, http_diagnostic, request_too_large_diagnostic,
    response_too_large_diagnostic, schema_diagnostic, stream_diagnostic, transport_diagnostic,
};
use crate::{
    MAX_PROVIDER_DIAGNOSTIC_BYTES, MAX_PROVIDER_ERROR_BODY_BYTES, MAX_PROVIDER_HTTP_BODY_BYTES,
    MAX_PROVIDER_HTTP_REQUEST_BYTES, MAX_PROVIDER_STREAM_FRAME_BYTES, ModelEvent, ModelEventStream,
    ProviderApiMode, ProviderError, ProviderEventStream, ProviderStreamEvent,
};
use async_stream::try_stream;
use futures::StreamExt;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use std::io::{self, Write};

pub(crate) fn encode_bounded_request(
    body: &Value,
    api_mode: ProviderApiMode,
) -> Result<Vec<u8>, ProviderError> {
    let mut writer = BoundedWriter::new(MAX_PROVIDER_HTTP_REQUEST_BYTES);
    if let Err(error) = serde_json::to_writer(&mut writer, body) {
        if writer.exceeded {
            return Err(ProviderError::Diagnostic(request_too_large_diagnostic(
                writer.attempted,
                api_mode,
            )));
        }
        return Err(ProviderError::Json(error));
    }
    Ok(writer.bytes)
}

pub(crate) fn encode_bounded_structured_value(value: &Value) -> Result<Vec<u8>, ProviderError> {
    let mut writer = BoundedWriter::new(MAX_PROVIDER_HTTP_BODY_BYTES);
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        if writer.exceeded {
            return Err(ProviderError::Diagnostic(response_too_large_diagnostic(
                MAX_PROVIDER_HTTP_BODY_BYTES,
                ProviderApiMode::Responses,
            )));
        }
        return Err(ProviderError::Json(error));
    }
    Ok(writer.bytes)
}

pub(crate) fn ensure_content_type(
    response: &reqwest::Response,
    allowed: &[&str],
    expected: &str,
    streaming: bool,
    api_mode: ProviderApiMode,
) -> Result<(), ProviderError> {
    let observed = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let media_type = observed
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    if media_type
        .as_deref()
        .is_some_and(|media_type| allowed.contains(&media_type))
    {
        return Ok(());
    }
    Err(ProviderError::Diagnostic(content_type_diagnostic(
        observed, expected, streaming, api_mode,
    )))
}

pub(crate) async fn read_bounded_body(
    response: reqwest::Response,
    limit: usize,
    api_mode: ProviderApiMode,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk
            .map_err(|error| ProviderError::Diagnostic(transport_diagnostic(&error, api_mode)))?;
        let Some(total) = body.len().checked_add(chunk.len()) else {
            return Err(ProviderError::Diagnostic(response_too_large_diagnostic(
                limit, api_mode,
            )));
        };
        if total > limit {
            return Err(ProviderError::Diagnostic(response_too_large_diagnostic(
                limit, api_mode,
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) async fn bounded_http_failure(
    response: reqwest::Response,
    api_mode: ProviderApiMode,
) -> ProviderError {
    let status = response.status().as_u16();
    let (body, truncated) = read_error_body(response, api_mode).await;
    ProviderError::Diagnostic(http_diagnostic(status, &body, truncated, api_mode))
}

pub(crate) fn parse_json_body(
    bytes: &[u8],
    api_mode: ProviderApiMode,
) -> Result<Value, ProviderError> {
    serde_json::from_slice(bytes).map_err(|_| {
        ProviderError::Diagnostic(schema_diagnostic(
            "Provider response body was not valid JSON",
            Some(bytes),
            false,
            api_mode,
        ))
    })
}

pub(crate) fn extract_chat_output(
    response: Value,
    api_mode: ProviderApiMode,
) -> Result<Value, ProviderError> {
    let encoded = bounded_value_excerpt(&response);
    let content = response["choices"]
        .as_array()
        .and_then(|choices| choices.first())
        .and_then(|choice| choice["message"]["content"].as_str())
        .ok_or_else(|| {
            ProviderError::Diagnostic(schema_diagnostic(
                "Chat response contained no message content",
                Some(&encoded),
                false,
                api_mode,
            ))
        })?;
    parse_structured_content(content, api_mode)
}

pub(crate) fn extract_output_json(
    response: Value,
    api_mode: ProviderApiMode,
) -> Result<Value, ProviderError> {
    let encoded = bounded_value_excerpt(&response);
    let text = response["output"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["type"] == "message"))
        .and_then(|message| message["content"].as_array())
        .and_then(|parts| parts.iter().find(|part| part["type"] == "output_text"))
        .and_then(|part| part["text"].as_str())
        .ok_or_else(|| {
            ProviderError::Diagnostic(schema_diagnostic(
                "Structured response contained no output text",
                Some(&encoded),
                false,
                api_mode,
            ))
        })?;
    parse_structured_content(text, api_mode)
}

pub(crate) fn extract_ollama_output(
    response: Value,
    api_mode: ProviderApiMode,
) -> Result<Value, ProviderError> {
    let encoded = bounded_value_excerpt(&response);
    if response.get("error").is_some() {
        return Err(ProviderError::Diagnostic(http_diagnostic(
            400, &encoded, false, api_mode,
        )));
    }
    let content = response
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::Diagnostic(schema_diagnostic(
                "Ollama native response contained no message content",
                Some(&encoded),
                false,
                api_mode,
            ))
        })?;
    parse_structured_content(content, api_mode)
}

pub(crate) fn ollama_native_stream(
    response: reqwest::Response,
    api_mode: ProviderApiMode,
) -> ModelEventStream {
    model_events_only(ollama_provider_stream(response, api_mode))
}

pub(crate) fn ollama_provider_stream(
    response: reqwest::Response,
    api_mode: ProviderApiMode,
) -> ProviderEventStream {
    Box::pin(try_stream! {
        let mut decoder = BoundedLineDecoder::default();
        let mut chunks = response.bytes_stream();
        let mut finished = false;
        yield ProviderStreamEvent::Connected;
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                ProviderError::Diagnostic(transport_diagnostic(&error, api_mode))
            })?;
            if !chunk.is_empty() {
                yield ProviderStreamEvent::BytesReceived {
                    byte_count: chunk.len(),
                };
            }
            for line in decoder.push(&chunk, api_mode)? {
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                for event in parse_ollama_stream_frame(&line, api_mode)? {
                    let terminal = event == ModelEvent::Finished;
                    yield ProviderStreamEvent::Model(event);
                    if terminal {
                        finished = true;
                    }
                }
                if finished {
                    break;
                }
            }
            if finished {
                break;
            }
        }
        if !finished
            && let Some(line) = decoder.finish()
            && !line.iter().all(u8::is_ascii_whitespace)
        {
            for event in parse_ollama_stream_frame(&line, api_mode)? {
                let terminal = event == ModelEvent::Finished;
                yield ProviderStreamEvent::Model(event);
                if terminal {
                    finished = true;
                }
            }
        }
        if !finished {
            Err(ProviderError::Diagnostic(stream_diagnostic(
                "Ollama stream ended before a done frame",
                None,
                false,
                api_mode,
            )))?;
        }
    })
}

pub(crate) fn openai_event_stream(
    response: reqwest::Response,
    api_mode: ProviderApiMode,
) -> ModelEventStream {
    model_events_only(openai_provider_stream(response, api_mode))
}

pub(crate) fn openai_provider_stream(
    response: reqwest::Response,
    api_mode: ProviderApiMode,
) -> ProviderEventStream {
    Box::pin(try_stream! {
        let mut decoder = SseDecoder::default();
        let mut chunks = response.bytes_stream();
        let mut finished = false;
        yield ProviderStreamEvent::Connected;
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                ProviderError::Diagnostic(transport_diagnostic(&error, api_mode))
            })?;
            if !chunk.is_empty() {
                yield ProviderStreamEvent::BytesReceived {
                    byte_count: chunk.len(),
                };
            }
            for data in decoder.push(&chunk, api_mode)? {
                for event in parse_openai_stream_frame(&data, api_mode)? {
                    let terminal = event == ModelEvent::Finished;
                    yield ProviderStreamEvent::Model(event);
                    if terminal {
                        finished = true;
                    }
                }
                if finished {
                    break;
                }
            }
            if finished {
                break;
            }
        }
        if !finished {
            for data in decoder.finish(api_mode)? {
                for event in parse_openai_stream_frame(&data, api_mode)? {
                    let terminal = event == ModelEvent::Finished;
                    yield ProviderStreamEvent::Model(event);
                    if terminal {
                        finished = true;
                    }
                }
            }
        }
        if !finished {
            Err(ProviderError::Diagnostic(stream_diagnostic(
                "Provider event stream ended before a terminal frame",
                None,
                false,
                api_mode,
            )))?;
        }
    })
}

fn model_events_only(stream: ProviderEventStream) -> ModelEventStream {
    Box::pin(stream.filter_map(|event| async move {
        match event {
            Ok(ProviderStreamEvent::Model(event)) => Some(Ok(event)),
            Ok(ProviderStreamEvent::Connected | ProviderStreamEvent::BytesReceived { .. }) => None,
            Err(error) => Some(Err(error)),
        }
    }))
}

#[cfg(test)]
pub(crate) fn parse_response_event(data: &str) -> Result<Option<ModelEvent>, ProviderError> {
    Ok(parse_response_events(data)?.into_iter().next())
}

#[cfg(test)]
pub(crate) fn parse_chat_event(data: &str) -> Result<Option<ModelEvent>, ProviderError> {
    Ok(parse_chat_events(data)?.into_iter().next())
}

fn parse_openai_stream_frame(
    data: &[u8],
    api_mode: ProviderApiMode,
) -> Result<Vec<ModelEvent>, ProviderError> {
    let data = std::str::from_utf8(data).map_err(|_| {
        ProviderError::Diagnostic(stream_diagnostic(
            "Provider stream frame was not UTF-8",
            None,
            false,
            api_mode,
        ))
    })?;
    if data.trim() == "[DONE]" {
        return Ok(vec![ModelEvent::Finished]);
    }
    match api_mode {
        ProviderApiMode::Responses => parse_response_events(data),
        ProviderApiMode::OpenaiCompatible => parse_chat_events(data),
        ProviderApiMode::OllamaNative => Err(ProviderError::Diagnostic(stream_diagnostic(
            "Ollama native mode cannot parse server-sent events",
            Some(data.as_bytes()),
            false,
            api_mode,
        ))),
    }
}

fn parse_response_events(data: &str) -> Result<Vec<ModelEvent>, ProviderError> {
    let api_mode = ProviderApiMode::Responses;
    let value = parse_stream_json(data, api_mode)?;
    let event_type = value["type"].as_str().unwrap_or_default();
    let mut events = Vec::new();
    match event_type {
        "response.created" => {
            if let Some(id) = value["response"]["id"].as_str() {
                events.push(ModelEvent::ResponseStarted {
                    response_id: id.into(),
                });
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = value["delta"].as_str() {
                events.push(ModelEvent::TextDelta(delta.into()));
            }
        }
        "response.output_item.done" if value["item"]["type"] == "function_call" => {
            events.push(ModelEvent::ToolCall {
                call_id: required_string(&value["item"], "call_id", data, api_mode)?,
                name: required_string(&value["item"], "name", data, api_mode)?,
                arguments: required_string(&value["item"], "arguments", data, api_mode)?,
            });
        }
        "response.completed" => {
            if let (Some(input), Some(output)) = (
                value["response"]["usage"]["input_tokens"].as_u64(),
                value["response"]["usage"]["output_tokens"].as_u64(),
            ) {
                events.push(ModelEvent::Usage {
                    input_tokens: input,
                    output_tokens: output,
                });
            }
            events.push(ModelEvent::Finished);
        }
        "response.failed" | "error" => {
            return Err(ProviderError::Diagnostic(http_diagnostic(
                400,
                data.as_bytes(),
                false,
                api_mode,
            )));
        }
        _ => {}
    }
    Ok(events)
}

fn parse_chat_events(data: &str) -> Result<Vec<ModelEvent>, ProviderError> {
    let api_mode = ProviderApiMode::OpenaiCompatible;
    let value = parse_stream_json(data, api_mode)?;
    let choices = value["choices"]
        .as_array()
        .and_then(|choices| choices.first());
    let delta = choices.and_then(|choice| choice["delta"].as_object());
    let finish_reason = choices.and_then(|choice| choice["finish_reason"].as_str());
    let mut events = Vec::new();
    if let Some(delta) = delta {
        if let Some(content) = delta.get("content").and_then(Value::as_str)
            && !content.is_empty()
        {
            events.push(ModelEvent::TextDelta(content.into()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (index, call) in calls.iter().enumerate() {
                events.push(ModelEvent::ToolCall {
                    call_id: call["id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("chat-tool-{index}")),
                    name: call["function"]["name"].as_str().unwrap_or("").into(),
                    arguments: call["function"]["arguments"]
                        .as_str()
                        .unwrap_or("{}")
                        .into(),
                });
            }
        }
    }
    if finish_reason.is_some() {
        if let (Some(input), Some(output)) = (
            value["usage"]["prompt_tokens"].as_u64(),
            value["usage"]["completion_tokens"].as_u64(),
        ) {
            events.push(ModelEvent::Usage {
                input_tokens: input,
                output_tokens: output,
            });
        }
        events.push(ModelEvent::Finished);
    }
    Ok(events)
}

fn parse_ollama_stream_frame(
    frame: &[u8],
    api_mode: ProviderApiMode,
) -> Result<Vec<ModelEvent>, ProviderError> {
    let value = serde_json::from_slice::<Value>(frame).map_err(|_| {
        ProviderError::Diagnostic(stream_diagnostic(
            "Ollama NDJSON frame was not valid JSON",
            Some(frame),
            false,
            api_mode,
        ))
    })?;
    if value.get("error").is_some() {
        return Err(ProviderError::Diagnostic(http_diagnostic(
            400, frame, false, api_mode,
        )));
    }
    if value.get("choices").is_some() || value.get("object").is_some() {
        return Err(ProviderError::Diagnostic(schema_diagnostic(
            "Ollama native stream returned an incompatible payload",
            Some(frame),
            false,
            api_mode,
        )));
    }
    let mut events = Vec::new();
    if let Some(content) = value.pointer("/message/content").and_then(Value::as_str)
        && !content.is_empty()
    {
        events.push(ModelEvent::TextDelta(content.into()));
    }
    if let Some(calls) = value
        .pointer("/message/tool_calls")
        .and_then(Value::as_array)
    {
        for (index, call) in calls.iter().enumerate() {
            let arguments = match &call["function"]["arguments"] {
                Value::String(arguments) => arguments.clone(),
                arguments => serde_json::to_string(arguments).map_err(ProviderError::Json)?,
            };
            events.push(ModelEvent::ToolCall {
                call_id: call["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("ollama-tool-{index}")),
                name: call["function"]["name"].as_str().unwrap_or("").into(),
                arguments,
            });
        }
    }
    if value["done"].as_bool() == Some(true) {
        if let (Some(input), Some(output)) = (
            value["prompt_eval_count"].as_u64(),
            value["eval_count"].as_u64(),
        ) {
            events.push(ModelEvent::Usage {
                input_tokens: input,
                output_tokens: output,
            });
        }
        events.push(ModelEvent::Finished);
    }
    if events.is_empty() && value["done"].as_bool().is_none() {
        return Err(ProviderError::Diagnostic(schema_diagnostic(
            "Ollama NDJSON frame lacked message and done fields",
            Some(frame),
            false,
            api_mode,
        )));
    }
    Ok(events)
}

fn parse_stream_json(data: &str, api_mode: ProviderApiMode) -> Result<Value, ProviderError> {
    serde_json::from_str(data).map_err(|_| {
        ProviderError::Diagnostic(stream_diagnostic(
            "Provider stream frame was not valid JSON",
            Some(data.as_bytes()),
            false,
            api_mode,
        ))
    })
}

fn required_string(
    value: &Value,
    field: &str,
    frame: &str,
    api_mode: ProviderApiMode,
) -> Result<String, ProviderError> {
    value[field].as_str().map(str::to_owned).ok_or_else(|| {
        ProviderError::Diagnostic(schema_diagnostic(
            &format!("Provider event omitted string field `{field}`"),
            Some(frame.as_bytes()),
            false,
            api_mode,
        ))
    })
}

fn parse_structured_content(
    content: &str,
    api_mode: ProviderApiMode,
) -> Result<Value, ProviderError> {
    serde_json::from_str(content).map_err(|_| {
        ProviderError::Diagnostic(schema_diagnostic(
            "Provider structured message content was not valid JSON",
            Some(content.as_bytes()),
            false,
            api_mode,
        ))
    })
}

async fn read_error_body(
    response: reqwest::Response,
    _api_mode: ProviderApiMode,
) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    let mut truncated = false;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let Ok(chunk) = chunk else {
            truncated = true;
            break;
        };
        let remaining = MAX_PROVIDER_ERROR_BODY_BYTES.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() == MAX_PROVIDER_ERROR_BODY_BYTES {
            truncated = true;
            break;
        }
    }
    (body, truncated)
}

fn bounded_value_excerpt(value: &Value) -> Vec<u8> {
    let mut writer = BoundedWriter::new(MAX_PROVIDER_DIAGNOSTIC_BYTES);
    let _ = serde_json::to_writer(&mut writer, value);
    writer.bytes
}

#[derive(Default)]
struct BoundedLineDecoder {
    line: Vec<u8>,
}

impl BoundedLineDecoder {
    fn push(
        &mut self,
        chunk: &[u8],
        api_mode: ProviderApiMode,
    ) -> Result<Vec<Vec<u8>>, ProviderError> {
        let mut lines = Vec::new();
        for byte in chunk {
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                lines.push(line);
            } else {
                if self.line.len() == MAX_PROVIDER_STREAM_FRAME_BYTES {
                    return Err(ProviderError::Diagnostic(stream_diagnostic(
                        "Provider stream frame exceeded the gateway limit",
                        Some(&self.line),
                        true,
                        api_mode,
                    )));
                }
                self.line.push(*byte);
            }
        }
        Ok(lines)
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        if self.line.is_empty() {
            None
        } else {
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            Some(line)
        }
    }
}

#[derive(Default)]
struct SseDecoder {
    lines: BoundedLineDecoder,
    data: Vec<u8>,
    frame_bytes: usize,
}

impl SseDecoder {
    fn push(
        &mut self,
        chunk: &[u8],
        api_mode: ProviderApiMode,
    ) -> Result<Vec<Vec<u8>>, ProviderError> {
        let lines = self.lines.push(chunk, api_mode)?;
        let mut frames = Vec::new();
        for line in lines {
            self.process_line(line, api_mode, &mut frames)?;
        }
        Ok(frames)
    }

    fn finish(&mut self, api_mode: ProviderApiMode) -> Result<Vec<Vec<u8>>, ProviderError> {
        let mut frames = Vec::new();
        if let Some(line) = self.lines.finish() {
            self.process_line(line, api_mode, &mut frames)?;
        }
        self.process_line(Vec::new(), api_mode, &mut frames)?;
        Ok(frames)
    }

    fn process_line(
        &mut self,
        line: Vec<u8>,
        api_mode: ProviderApiMode,
        frames: &mut Vec<Vec<u8>>,
    ) -> Result<(), ProviderError> {
        self.frame_bytes = self
            .frame_bytes
            .checked_add(line.len().saturating_add(1))
            .ok_or_else(|| {
                ProviderError::Diagnostic(stream_diagnostic(
                    "Provider SSE frame size overflowed",
                    Some(&self.data),
                    true,
                    api_mode,
                ))
            })?;
        if self.frame_bytes > MAX_PROVIDER_STREAM_FRAME_BYTES {
            return Err(ProviderError::Diagnostic(stream_diagnostic(
                "Provider SSE frame exceeded the gateway limit",
                Some(&self.data),
                true,
                api_mode,
            )));
        }
        if line.is_empty() {
            if self.data.last() == Some(&b'\n') {
                self.data.pop();
            }
            if !self.data.is_empty() {
                frames.push(std::mem::take(&mut self.data));
            }
            self.frame_bytes = 0;
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            let projected = self
                .data
                .len()
                .saturating_add(value.len().saturating_add(1));
            if projected > MAX_PROVIDER_STREAM_FRAME_BYTES {
                return Err(ProviderError::Diagnostic(stream_diagnostic(
                    "Provider SSE data exceeded the gateway limit",
                    Some(&self.data),
                    true,
                    api_mode,
                )));
            }
            self.data.extend_from_slice(value);
            self.data.push(b'\n');
        }
        Ok(())
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    maximum: usize,
    attempted: usize,
    exceeded: bool,
}

impl BoundedWriter {
    fn new(maximum: usize) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            attempted: 0,
            exceeded: false,
        }
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.attempted = self.attempted.saturating_add(buffer.len());
        let remaining = self.maximum.saturating_sub(self.bytes.len());
        if buffer.len() > remaining {
            self.bytes.extend_from_slice(&buffer[..remaining]);
            self.exceeded = true;
            return Err(io::Error::other("bounded JSON writer limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
