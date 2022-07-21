/*!
A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides integrations for both `hyper` and `actix-web` (1.x, 2.x, 3.x, 4.x).
Enable features `actix-web-v3-integration`, `hyper-integration`, etc. as needed.

`actix-web-v4-integration` is enabled by default. Make sure to add `default-features = false` if using `hyper` or other `actix-web` versions.

Also see the `easy-errors` feature flag (not enabled by default). Enabling this flag will implement [`ErrorLike`](https://docs.rs/jsonrpc-v2/&#42;/jsonrpc_v2/trait.ErrorLike.html)
for anything that implements `Display`, and the display value will be provided in the `message` field of the JSON-RPC 2.0 `Error` response.

Otherwise, custom errors should implement [`ErrorLike`](https://docs.rs/jsonrpc-v2/&#42;/jsonrpc_v2/trait.ErrorLike.html) to map errors to the JSON-RPC 2.0 `Error` response.

Individual method handlers are `async` functions that can take various kinds of args (things that can be extracted from the request, like
the `Params` or `Data`), and should return a `Result<Item, Error>` where the `Item` is serializable. See examples below.

# Usage

```rust,no_run
use jsonrpc_v2::{Data, Error, Params, Server};

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

async fn add(Params(params): Params<TwoNums>) -> Result<usize, Error> {
    Ok(params.a + params.b)
}

async fn sub(Params(params): Params<(usize, usize)>) -> Result<usize, Error> {
    Ok(params.0 - params.1)
}

async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("sub", sub)
        .with_method("message", message)
        .finish();

    actix_web::HttpServer::new(move || {
        let rpc = rpc.clone();
        actix_web::App::new().service(
            actix_web::web::service("/api")
                .guard(actix_web::guard::Post())
                .finish(rpc.into_web_service()),
        )
    })
    .bind("0.0.0.0:3000")?
    .run()
    .await
}
```
*/

use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};

use serde_json::{value::RawValue, Value};

use std::sync::Arc;

use futures::{
    future::{self, Future, FutureExt, TryFutureExt},
    stream::{StreamExt, TryStreamExt},
};

// #[cfg(any(
//     feature = "actix-web-v1",
//     feature = "actix-web-v2",
//     feature = "actix-web-v3",
//     feature = "actix-web-v4"
// ))]
// use futures::future::TryFutureExt;

// #[cfg(any(feature = "actix-web-v2", feature = "actix-web-v3", feature = "actix-web-v4"))]
// use futures::stream::TryStreamExt;

use std::{collections::HashMap, marker::PhantomData};

#[cfg(feature = "bytes-v10")]
use bytes_v10::Bytes;

#[cfg(feature = "bytes-v05")]
use bytes_v05::Bytes;

#[cfg(feature = "bytes-v04")]
use bytes_v04::Bytes;

#[cfg(any(
    feature = "actix-web-v1",
    feature = "actix-web-v2",
    feature = "actix-web-v3",
    feature = "actix-web-v4"
))]
mod actix_web;

#[cfg(any(
    feature = "actix-web-v1",
    feature = "actix-web-v2",
    feature = "actix-web-v3",
    feature = "actix-web-v4"
))]
pub use crate::actix_web::*;

#[cfg(feature = "hyper-integration")]
mod hyper_integration;

#[cfg(feature = "hyper-integration")]
pub use crate::hyper_integration::*;

// #[cfg(feature = "macros")]
// pub use jsonrpc_v2_macros::jsonrpc_v2_method;

use type_map::concurrent::TypeMap;

// #[cfg(feature = "macros")]
// pub mod exp {
//     pub use serde;
//     pub use serde_json;
// }

#[derive(Clone)]
pub(crate) struct HttpRequestWrapper(pub HttpRequest);

type BoxedSerialize = Box<dyn erased_serde::Serialize + Send>;

/// Error object in a response
#[derive(Serialize)]
#[serde(untagged)]
pub enum Error {
    Full {
        code: i64,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<BoxedSerialize>,
    },
    Provided {
        code: i64,
        message: &'static str,
    },
}

impl Error {
    pub const INVALID_REQUEST: Self = Error::Provided { code: -32600, message: "Invalid Request" };
    pub const METHOD_NOT_FOUND: Self =
        Error::Provided { code: -32601, message: "Method not found" };
    pub const INVALID_PARAMS: Self = Error::Provided { code: -32602, message: "Invalid params" };
    pub const INTERNAL_ERROR: Self = Error::Provided { code: -32603, message: "Internal Error" };
    pub const PARSE_ERROR: Self = Error::Provided { code: -32700, message: "Parse error" };

    pub fn internal<D: std::fmt::Display + Send>(e: D) -> Self {
        Error::Full {
            code: -32603,
            message: "Internal Error".into(),
            data: Some(Box::new(e.to_string())),
        }
    }
}

/// Trait that can be used to map custom errors to the [`Error`](enum.Error.html) object.
pub trait ErrorLike: std::fmt::Display {
    /// Code to be used in JSON-RPC 2.0 Error object. Default is 0.
    fn code(&self) -> i64 {
        0
    }

    /// Message to be used in JSON-RPC 2.0 Error object. Default is the `Display` value of the item.
    fn message(&self) -> String {
        self.to_string()
    }

    /// Any additional data to be sent with the error. Default is `None`.
    fn data(&self) -> Option<BoxedSerialize> {
        None
    }
}

impl<T> From<T> for Error
where
    T: ErrorLike,
{
    fn from(t: T) -> Error {
        Error::Full { code: t.code(), message: t.message(), data: t.data() }
    }
}

#[cfg(feature = "easy-errors")]
impl<T> ErrorLike for T where T: std::fmt::Display {}

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct V2;

impl Serialize for V2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        "2.0".serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for V2 {
    fn deserialize<D>(deserializer: D) -> Result<V2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: &str = Deserialize::deserialize(deserializer)?;
        if s == "2.0" {
            Ok(V2)
        } else {
            Err(serde::de::Error::custom("Could not deserialize V2"))
        }
    }
}

/// Container for the request ID, which can be a string, number, or null.
/// Not typically used directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Num(i64),
    Str(Box<str>),
    Null,
}

impl From<i64> for Id {
    fn from(t: i64) -> Self {
        Id::Num(t)
    }
}

impl<'a> From<&'a str> for Id {
    fn from(t: &'a str) -> Self {
        Id::Str(t.into())
    }
}

impl From<String> for Id {
    fn from(t: String) -> Self {
        Id::Str(t.into())
    }
}

impl Default for Id {
    fn default() -> Self {
        Id::Null
    }
}

/// Method string wrapper, for `FromRequest` extraction
#[derive(Debug)]
pub struct Method(Box<str>);

impl Method {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Method> for String {
    fn from(t: Method) -> String {
        String::from(t.0)
    }
}

/// Builder struct for a request object
#[derive(Default)]
pub struct RequestBuilder<M = ()> {
    id: Id,
    params: Option<Value>,
    method: M,
}

impl<M> RequestBuilder<M> {
    pub fn with_id<I: Into<Id>>(mut self, id: I) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_params<I: Into<Value>>(mut self, params: I) -> Self {
        self.params = Some(params.into());
        self
    }
}

impl RequestBuilder<()> {
    pub fn with_method<I: Into<String>>(self, method: I) -> RequestBuilder<String> {
        let RequestBuilder { id, params, .. } = self;
        RequestBuilder { id, params, method: method.into() }
    }
}

impl RequestBuilder<String> {
    pub fn finish(self) -> RequestObject {
        let RequestBuilder { id, params, method } = self;
        RequestObject {
            jsonrpc: V2,
            method: method.into_boxed_str(),
            params: params.map(InnerParams::Value),
            id: Some(Some(id)),
        }
    }
}

/// Builder struct for a notification request object
#[derive(Default)]
pub struct NotificationBuilder<M = ()> {
    params: Option<Value>,
    method: M,
}

impl<M> NotificationBuilder<M> {
    pub fn with_params<I: Into<Value>>(mut self, params: I) -> Self {
        self.params = Some(params.into());
        self
    }
}

impl NotificationBuilder<()> {
    pub fn with_method<I: Into<String>>(self, method: I) -> NotificationBuilder<String> {
        let NotificationBuilder { params, .. } = self;
        NotificationBuilder { params, method: method.into() }
    }
}

impl NotificationBuilder<String> {
    pub fn finish(self) -> RequestObject {
        let NotificationBuilder { method, params } = self;
        RequestObject {
            jsonrpc: V2,
            method: method.into_boxed_str(),
            params: params.map(InnerParams::Value),
            id: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum InnerParams {
    Value(Value),
    Raw(Box<RawValue>),
}

/// Request/Notification object
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct RequestObject {
    jsonrpc: V2,
    method: Box<str>,
    params: Option<InnerParams>,
    #[serde(deserialize_with = "RequestObject::deserialize_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Option<Id>>,
}

impl RequestObject {
    pub fn method_ref(&self) -> &str {
        &self.method
    }

    pub fn id_ref(&self) -> Option<&Id> {
        self.id.as_ref().and_then(|x| x.as_ref())
    }
}

/// Request/Notification object
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct BytesRequestObject {
    jsonrpc: V2,
    method: Box<str>,
    params: Option<Box<RawValue>>,
    #[serde(deserialize_with = "RequestObject::deserialize_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Option<Id>>,
}

impl From<BytesRequestObject> for RequestObject {
    fn from(t: BytesRequestObject) -> Self {
        let BytesRequestObject { jsonrpc, method, params, id } = t;
        RequestObject { jsonrpc, method, params: params.map(InnerParams::Raw), id }
    }
}

impl RequestObject {
    /// Build a new request object
    pub fn request() -> RequestBuilder {
        RequestBuilder::default()
    }

    /// Build a new notification request object
    pub fn notification() -> NotificationBuilder {
        NotificationBuilder::default()
    }

    fn deserialize_id<'de, D>(deserializer: D) -> Result<Option<Option<Id>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Some(Option::deserialize(deserializer)?))
    }
}


#[doc(hidden)]
pub struct RequestObjectWithData {
    inner: RequestObject,
    data: Option<Arc<TypeMap>>,
    http_request_local_data: Option<Arc<TypeMap>>,
}

/// [`FromRequest`](trait.FromRequest.html) wrapper for request params
///
/// Use a tuple to deserialize by-position params
/// and a map or deserializable struct for by-name params: e.g.
///
/// ```rust,no_run
/// fn handler(params: Params<(i32, String)>) -> Result<String, Error> { /* ... */ }
/// ```
#[derive(Deserialize)]
pub struct Params<T>(pub T);

impl<T> Params<T>
where
    T: DeserializeOwned,
{
    fn from_request_object(req: &RequestObject) -> Result<Self, Error> {
        let res = match req.params {
            Some(InnerParams::Raw(ref value)) => serde_json::from_str(value.get()),
            Some(InnerParams::Value(ref value)) => serde_json::from_value(value.clone()),
            None => serde_json::from_value(Value::Null),
        };
        res.map(Params).map_err(|_| Error::INVALID_PARAMS)
    }
}

/// Data/state storage container
pub struct Data<T>(pub Arc<T>);

impl<T> Data<T> {
    pub fn new(t: T) -> Self {
        Data(Arc::new(t))
    }
}

impl<T> std::ops::Deref for Data<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// Data/state storage container
pub struct HttpRequestLocalData<T>(pub Arc<T>);

impl<T> std::ops::Deref for HttpRequestLocalData<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// The individual response object
#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseObject {
    Result { jsonrpc: V2, result: BoxedSerialize, id: Id },
    Error { jsonrpc: V2, error: Error, id: Id },
}

impl ResponseObject {
    fn result(result: BoxedSerialize, id: Id) -> Self {
        ResponseObject::Result { jsonrpc: V2, result, id }
    }

    fn error(error: Error, id: Id) -> Self {
        ResponseObject::Error { jsonrpc: V2, error, id }
    }
}

/// Container for the response object(s) or `Empty` for notification request(s)
#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseObjects {
    One(ResponseObject),
    Many(Vec<ResponseObject>),
    Empty,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ManyResponseObjects {
    Many(Vec<ResponseObject>),
    Empty,
}

#[derive(Serialize)]
#[serde(untagged)]
enum SingleResponseObject {
    One(ResponseObject),
    Empty,
}

impl From<ManyResponseObjects> for ResponseObjects {
    fn from(t: ManyResponseObjects) -> Self {
        match t {
            ManyResponseObjects::Many(many) => ResponseObjects::Many(many),
            ManyResponseObjects::Empty => ResponseObjects::Empty,
        }
    }
}

impl From<SingleResponseObject> for ResponseObjects {
    fn from(t: SingleResponseObject) -> Self {
        match t {
            SingleResponseObject::One(one) => ResponseObjects::One(one),
            SingleResponseObject::Empty => ResponseObjects::Empty,
        }
    }
}

impl SingleResponseObject {
    fn result(result: BoxedSerialize, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| SingleResponseObject::One(ResponseObject::result(result, id)))
            .unwrap_or_else(|| SingleResponseObject::Empty)
    }

    fn error(error: Error, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| SingleResponseObject::One(ResponseObject::error(error, id)))
            .unwrap_or_else(|| SingleResponseObject::Empty)
    }
}

/// An enum to contain the different kinds of possible requests: using the provided
/// [`RequestObject`](struct.RequestObject.html), an array of `RequestObject`s, or raw bytes.
///
/// Typically not use directly, [`Server::handle`](struct.Server.html#method.handle) can take the individual variants
pub enum RequestKind {
    RequestObject(RequestObject),
    ManyRequestObjects(Vec<RequestObject>),
    Bytes(Bytes),
}

impl From<RequestObject> for RequestKind {
    fn from(t: RequestObject) -> Self {
        RequestKind::RequestObject(t)
    }
}

impl From<Vec<RequestObject>> for RequestKind {
    fn from(t: Vec<RequestObject>) -> Self {
        RequestKind::ManyRequestObjects(t)
    }
}

impl From<Bytes> for RequestKind {
    fn from(t: Bytes) -> Self {
        RequestKind::Bytes(t)
    }
}

impl<'a> From<&'a [u8]> for RequestKind {
    fn from(t: &'a [u8]) -> Self {
        Bytes::from(t.to_vec()).into()
    }
}

#[derive(Debug)]
enum OneOrManyRawValues<'a> {
    Many(Vec<&'a RawValue>),
    One(&'a RawValue),
}

impl<'a> OneOrManyRawValues<'a> {
    pub fn try_from_slice(slice: &'a [u8]) -> Result<Self, serde_json::Error> {
        if slice.first() == Some(&b'[') {
            Ok(OneOrManyRawValues::Many(serde_json::from_slice::<Vec<&RawValue>>(slice)?))
        } else {
            Ok(OneOrManyRawValues::One(serde_json::from_slice::<&RawValue>(slice)?))
        }
    }
}
