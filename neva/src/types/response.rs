//! Represents a response that MCP server provides

use crate::error::Error;
use crate::types::{JSONRPC_VERSION, Message, RequestId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(feature = "http-server")]
use http::HeaderMap;

pub use error_details::ErrorDetails;
pub use into_response::IntoResponse;

mod error_details;
mod into_response;

/// The `resultType` discriminator MCP 2026-07-28 puts on every result.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const RESULT_TYPE: &str = "resultType";

/// The `resultType` value marking a result as final.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const COMPLETE: &str = "complete";

/// The `resultType` value marking a result as an MRTR continuation.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const INPUT_REQUIRED: &str = "input_required";

/// The `resultType` value marking a result the server deferred onto a task.
#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
pub(crate) const TASK: &str = "task";

/// Discriminator carried by every MCP 2026-07-28 result.
///
/// The spec makes `resultType` mandatory on results, but keeps an absent field
/// readable as [`ResultType::Complete`] so a peer speaking an older revision
/// still parses. neva applies that rule on the way in
/// ([`Response::result_type`]) and emits the field on the way out.
///
/// # Examples
///
/// ```
/// use neva::types::{RequestId, Response, ResultType};
///
/// let resp = Response::success(RequestId::Number(1), serde_json::json!({}));
/// assert_eq!(resp.result_type(), Some(ResultType::Complete));
/// ```
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultType {
    /// `"complete"` -- the result is final. Also what an absent field means.
    #[serde(rename = "complete")]
    Complete,

    /// `"input_required"` -- the server needs more input; see
    /// [`InputRequiredResult`](crate::types::mrtr::InputRequiredResult).
    #[serde(rename = "input_required")]
    InputRequired,

    /// `"task"` -- the server deferred the request onto a task instead of
    /// answering inline; see [`CreateTaskResult`](crate::types::CreateTaskResult).
    #[cfg(feature = "tasks")]
    #[serde(rename = "task")]
    Task,
}

/// Stamps `resultType: "complete"` onto a result object that does not already
/// carry a discriminator.
///
/// Non-object results (neva's scalar `IntoResponse` impls wrap those in an
/// object, but a hand-rolled handler may return a bare array) are passed
/// through untouched -- there is nowhere to put the field, and the spec only
/// describes object-shaped results.
#[cfg(not(feature = "legacy-spec"))]
#[inline]
fn tag_complete(mut result: Value) -> Value {
    if let Value::Object(map) = &mut result
        && !map.contains_key(RESULT_TYPE)
    {
        map.insert(RESULT_TYPE.into(), Value::String(COMPLETE.into()));
    }
    result
}

/// A response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    /// A successful response.
    Ok(OkResponse),

    /// A response that indicates an error occurred.
    Err(ErrorResponse),
}

/// A successful response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkResponse {
    /// JSON-RPC protocol version.
    ///
    /// > Note: always 2.0.
    pub jsonrpc: String,

    /// Request identifier matching the original request.
    #[serde(default)]
    pub id: RequestId,

    /// The result of the method invocation.
    pub result: Value,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,

    /// HTTP headers
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,
}

/// A response to a request that indicates an error occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// JSON-RPC protocol version.
    ///
    /// > Note: always 2.0.
    pub jsonrpc: String,

    /// Request identifier matching the original request.
    #[serde(default)]
    pub id: RequestId,

    /// Error information.
    pub error: ErrorDetails,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,

    /// HTTP headers
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,
}

impl From<Response> for Message {
    #[inline]
    fn from(response: Response) -> Self {
        Self::Response(response)
    }
}

impl Response {
    /// Creates a successful response
    // The `InputRequiredResult` link only resolves in a build that has MRTR, so
    // the whole paragraph is attached only there.
    #[cfg_attr(
        not(feature = "legacy-spec"),
        doc = "",
        doc = "Under MCP 2026-07-28 the result is stamped with",
        doc = "`resultType: \"complete\"` unless it already carries a discriminator --",
        doc = "which is how [`InputRequiredResult`](crate::types::mrtr::InputRequiredResult)",
        doc = "keeps its own `\"input_required\"` on the way out."
    )]
    pub fn success(id: RequestId, result: Value) -> Self {
        #[cfg(not(feature = "legacy-spec"))]
        let result = tag_complete(result);
        Response::Ok(OkResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            id,
            result,
        })
    }

    /// Creates a dummy successful response
    pub fn empty(id: RequestId) -> Self {
        #[cfg(not(feature = "legacy-spec"))]
        let result = json!({ RESULT_TYPE: COMPLETE });
        #[cfg(feature = "legacy-spec")]
        let result = json!({});
        Response::Ok(OkResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            #[cfg(feature = "http-server")]
            headers: HeaderMap::new(),
            id,
            result,
        })
    }

    /// Creates an error response
    pub fn error(id: RequestId, error: Error) -> Self {
        Response::Err(ErrorResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            id,
            error: error.into(),
        })
    }

    /// Returns the `resultType` discriminator of a successful result, or
    /// `None` for an error response.
    ///
    /// An **absent** field reads as [`ResultType::Complete`] -- the spec's
    /// backwards-compatibility rule, which is what lets a peer speaking an
    /// older revision interoperate. So does any value neva does not recognize:
    /// only `"input_required"` changes how a result is handled, and treating an
    /// unknown discriminator as final is the safe reading (the alternative is
    /// waiting for input nobody asked for).
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::{RequestId, Response, ResultType};
    ///
    /// // A legacy-shaped result without the field still reads as complete.
    /// let legacy = serde_json::from_str::<Response>(
    ///     r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#
    /// ).unwrap();
    /// assert_eq!(legacy.result_type(), Some(ResultType::Complete));
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn result_type(&self) -> Option<ResultType> {
        let Response::Ok(ok) = self else {
            return None;
        };
        Some(match ok.result.get(RESULT_TYPE).and_then(Value::as_str) {
            Some(INPUT_REQUIRED) => ResultType::InputRequired,
            #[cfg(feature = "tasks")]
            Some(TASK) => ResultType::Task,
            _ => ResultType::Complete,
        })
    }

    /// Returns [`Response`] ID
    pub fn id(&self) -> &RequestId {
        match &self {
            Response::Ok(ok) => &ok.id,
            Response::Err(err) => &err.id,
        }
    }

    /// Returns the full id (session_id?/response_id)
    pub fn full_id(&self) -> RequestId {
        let id = self.id().clone();
        if let Some(session_id) = self.session_id() {
            id.concat(RequestId::Uuid(*session_id))
        } else {
            id
        }
    }

    /// Set the `id` for the response
    pub fn set_id(mut self, id: RequestId) -> Self {
        match &mut self {
            Response::Ok(ok) => ok.id = id,
            Response::Err(err) => err.id = id,
        }
        self
    }

    /// Returns MCP Session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        match &self {
            Response::Ok(ok) => ok.session_id.as_ref(),
            Response::Err(err) => err.session_id.as_ref(),
        }
    }

    /// Set MCP `session_id` for the response
    pub fn set_session_id(mut self, id: uuid::Uuid) -> Self {
        match &mut self {
            Response::Ok(ok) => ok.session_id = Some(id),
            Response::Err(err) => err.session_id = Some(id),
        }
        self
    }

    /// Set HTTP headers for the response
    #[cfg(feature = "http-server")]
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        match &mut self {
            Response::Ok(ok) => ok.headers = headers,
            Response::Err(err) => err.headers = headers,
        }
        self
    }

    /// Unwraps the [`Response`] into either result of `T` or [`Error`]
    pub fn into_result<T: DeserializeOwned>(self) -> Result<T, Error> {
        match self {
            Response::Ok(ok) => serde_json::from_value::<T>(ok.result).map_err(Into::into),
            Response::Err(err) => Err(err.error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Response;
    use crate::{error::Error, types::RequestId};

    #[test]
    fn it_deserializes_successful_response_with_int_id_to_json() {
        let resp = Response::success(RequestId::Number(42), serde_json::json!({ "key": "test" }));

        let json = serde_json::to_string(&resp).unwrap();

        #[cfg(feature = "legacy-spec")]
        assert_eq!(json, r#"{"jsonrpc":"2.0","id":42,"result":{"key":"test"}}"#);
        #[cfg(not(feature = "legacy-spec"))]
        assert_eq!(
            json,
            r#"{"jsonrpc":"2.0","id":42,"result":{"key":"test","resultType":"complete"}}"#
        );
    }

    #[test]
    fn it_deserializes_error_response_with_string_id_to_json() {
        let resp = Response::error(
            RequestId::String("id".into()),
            Error::new(-32603, "some error message"),
        );

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(
            json,
            r#"{"jsonrpc":"2.0","id":"id","error":{"code":-32603,"message":"some error message","data":null}}"#
        );
    }
}

/// `resultType` -- the mandatory discriminator MCP 2026-07-28 puts on results.
#[cfg(test)]
#[cfg(not(feature = "legacy-spec"))]
mod result_type_tests {
    use super::{Response, ResultType};
    use crate::{error::Error, types::RequestId};

    fn parse(raw: &str) -> Response {
        serde_json::from_str(raw).expect("a well-formed JSON-RPC response")
    }

    #[test]
    fn every_success_result_is_stamped_complete() {
        let resp = Response::success(RequestId::Number(1), serde_json::json!({ "tools": [] }));
        let Response::Ok(ok) = &resp else {
            panic!("expected a success response")
        };

        assert_eq!(ok.result["resultType"], serde_json::json!("complete"));
        assert_eq!(resp.result_type(), Some(ResultType::Complete));
    }

    #[test]
    fn an_empty_result_is_stamped_too() {
        let resp = Response::empty(RequestId::Number(1));
        let Response::Ok(ok) = &resp else {
            panic!("expected a success response")
        };

        assert_eq!(ok.result, serde_json::json!({ "resultType": "complete" }));
    }

    #[test]
    fn an_existing_discriminator_is_never_overwritten() {
        // This is what keeps MRTR working: `InputRequiredResult` serializes its
        // own `"input_required"` and goes through the same `success` funnel.
        let resp = Response::success(
            RequestId::Number(1),
            serde_json::json!({ "resultType": "input_required", "requestState": "abc" }),
        );

        assert_eq!(resp.result_type(), Some(ResultType::InputRequired));
    }

    #[test]
    fn a_non_object_result_is_passed_through() {
        // Nowhere to put the field; the spec only describes object results.
        let resp = Response::success(RequestId::Number(1), serde_json::json!([1, 2, 3]));
        let Response::Ok(ok) = &resp else {
            panic!("expected a success response")
        };

        assert_eq!(ok.result, serde_json::json!([1, 2, 3]));
        assert_eq!(resp.result_type(), Some(ResultType::Complete));
    }

    #[test]
    fn a_legacy_shaped_result_without_the_field_reads_as_complete() {
        let resp = parse(r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#);

        assert_eq!(resp.result_type(), Some(ResultType::Complete));
    }

    #[test]
    fn an_unrecognized_discriminator_reads_as_complete() {
        // Only `"input_required"` changes how a result is handled. Anything
        // else is final -- the safe reading, since the alternative is blocking
        // on input nobody asked for.
        let resp = parse(r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"whatever"}}"#);

        assert_eq!(resp.result_type(), Some(ResultType::Complete));
    }

    #[test]
    fn an_error_response_has_no_result_type() {
        let resp = Response::error(RequestId::Number(1), Error::new(-32603, "boom"));

        assert_eq!(resp.result_type(), None);
    }

    #[test]
    fn the_discriminator_survives_a_wire_round_trip() {
        let resp = Response::success(RequestId::Number(1), serde_json::json!({ "tools": [] }));

        let back = parse(&serde_json::to_string(&resp).unwrap());

        assert_eq!(back.result_type(), Some(ResultType::Complete));
    }
}

/// Every result type neva can put on the wire carries the discriminator, and
/// still deserializes back into its own struct with the extra field present.
#[cfg(test)]
#[cfg(all(feature = "server", not(feature = "legacy-spec")))]
mod result_type_per_type_tests {
    use super::{Response, ResultType};
    use crate::types::{IntoResponse, RequestId};

    /// Round-trips `result` through `IntoResponse` and back into `T`.
    fn round_trip<T>(result: impl IntoResponse)
    where
        T: serde::de::DeserializeOwned,
    {
        let resp = result.into_response(RequestId::Number(1));

        assert_eq!(
            resp.result_type(),
            Some(ResultType::Complete),
            "result is missing the `complete` discriminator"
        );

        let wire = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&wire).unwrap();

        assert_eq!(back.result_type(), Some(ResultType::Complete));
        back.into_result::<T>()
            .expect("the typed result must still parse with `resultType` present");
    }

    #[test]
    fn tools_results_carry_it() {
        use crate::types::{CallToolResponse, ListToolsResult};

        round_trip::<ListToolsResult>(ListToolsResult::default());
        round_trip::<CallToolResponse>(CallToolResponse::new("ok"));
    }

    #[test]
    fn prompts_results_carry_it() {
        use crate::types::{GetPromptResult, ListPromptsResult};

        round_trip::<ListPromptsResult>(ListPromptsResult::default());
        round_trip::<GetPromptResult>(GetPromptResult::default());
    }

    #[test]
    fn resources_results_carry_it() {
        use crate::types::{ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult};

        round_trip::<ListResourcesResult>(ListResourcesResult::default());
        round_trip::<ListResourceTemplatesResult>(ListResourceTemplatesResult::default());
        round_trip::<ReadResourceResult>(ReadResourceResult::default());
    }

    #[test]
    fn completion_results_carry_it() {
        use crate::types::CompleteResult;

        round_trip::<CompleteResult>(CompleteResult::default());
    }

    #[test]
    fn discover_results_carry_it() {
        use crate::app::options::McpOptions;
        use crate::types::DiscoverResult;

        round_trip::<DiscoverResult>(DiscoverResult::new(&McpOptions::default()));
    }

    #[cfg(feature = "tasks")]
    #[test]
    fn task_results_carry_it() {
        use crate::types::{DetailedTask, Task, TaskPayload};

        round_trip::<DetailedTask>(DetailedTask::from(Task::new()));
        // A payload wrapping an object gets the field; a payload wrapping a
        // scalar has nowhere to put it and is passed through (see
        // `a_non_object_result_is_passed_through`).
        round_trip::<TaskPayload>(TaskPayload(serde_json::json!({ "content": [] })));
    }

    /// `CreateTaskResult` is the one result that is *not* `complete`: it is the
    /// third discriminator value, marking a request the server deferred.
    #[cfg(feature = "tasks")]
    #[test]
    fn a_created_task_is_tagged_task_not_complete() {
        use crate::types::{CreateTaskResult, Task};

        let resp = CreateTaskResult::new(Task::new()).into_response(RequestId::Number(1));

        assert_eq!(resp.result_type(), Some(ResultType::Task));

        // ...and the task's own fields sit at the top level, per `Result & Task`.
        let Response::Ok(ok) = &resp else {
            panic!("expected a success response")
        };
        assert!(ok.result.get("taskId").is_some(), "got: {}", ok.result);
        assert!(ok.result.get("task").is_none(), "must not be nested");
    }
}
