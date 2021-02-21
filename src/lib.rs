/*!
A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides integrations for both `hyper` and `actix-web` (both 1.x and 2.x).
Enable features `actix-web-v1-integration`, `actix-web-v2-integration`, or `hyper-integration` depending on need.

`actix-web-v2-integration` is enabled by default. Make sure to add `default-features = false` if using `hyper` or `actix-web` 1.x.

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
    future::{self, Future, FutureExt},
    stream::StreamExt,
};

#[cfg(any(feature = "actix-web-v1", feature = "actix-web-v2", feature = "actix-web-v3"))]
use futures::future::TryFutureExt;

#[cfg(any(feature = "actix-web-v2", feature = "actix-web-v3"))]
use futures::stream::TryStreamExt;

use extensions::concurrent::Extensions;
use std::{collections::HashMap, marker::PhantomData};

#[cfg(feature = "bytes-v10")]
use bytes_v10::Bytes;

#[cfg(feature = "bytes-v05")]
use bytes_v05::Bytes;

#[cfg(feature = "bytes-v04")]
use bytes_v04::Bytes;

#[cfg(feature = "macros")]
pub use jsonrpc_v2_macros::jsonrpc_v2_method;

#[cfg(feature = "macros")]
pub mod exp {
    pub use serde;
    pub use serde_json;
}

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
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RequestObject {
    jsonrpc: V2,
    method: Box<str>,
    params: Option<InnerParams>,
    #[serde(deserialize_with = "RequestObject::deserialize_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Option<Id>>,
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
    data: Arc<Extensions>,
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

/// A trait to extract data from the request
#[async_trait::async_trait]
pub trait FromRequest: Sized {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error>;
}

#[async_trait::async_trait]
impl FromRequest for () {
    async fn from_request(_: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(())
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

#[async_trait::async_trait]
impl<T: Send + Sync + 'static> FromRequest for Data<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let out = req.data.get::<Data<T>>().map(|x| Data(Arc::clone(&x.0))).ok_or_else(|| {
            Error::internal(format!("Missing data for: `{}`", std::any::type_name::<T>()))
        })?;
        Ok(out)
    }
}

#[async_trait::async_trait]
impl<T: DeserializeOwned> FromRequest for Params<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(Self::from_request_object(&req.inner)?)
    }
}

#[async_trait::async_trait]
impl FromRequest for Id {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(req
            .inner
            .id
            .clone()
            .and_then(|x| x)
            .ok_or_else(|| Error::internal("No `id` provided"))?)
    }
}

#[async_trait::async_trait]
impl FromRequest for Method {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(Method(req.inner.method.clone()))
    }
}

#[async_trait::async_trait]
impl<T: FromRequest> FromRequest for Option<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(T::from_request(req).await.ok())
    }
}

#[async_trait::async_trait]
impl<T1> FromRequest for (T1,)
where
    T1: FromRequest + Send,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok((T1::from_request(req).await?,))
    }
}

#[async_trait::async_trait]
impl<T1, T2> FromRequest for (T1, T2)
where
    T1: FromRequest + Send,
    T2: FromRequest + Send,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2) = futures::join!(T1::from_request(req), T2::from_request(req));
        Ok((t1?, t2?))
    }
}

#[async_trait::async_trait]
impl<T1, T2, T3> FromRequest for (T1, T2, T3)
where
    T1: FromRequest + Send,
    T2: FromRequest + Send,
    T3: FromRequest + Send,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3) =
            futures::join!(T1::from_request(req), T2::from_request(req), T3::from_request(req));
        Ok((t1?, t2?, t3?))
    }
}

#[async_trait::async_trait]
impl<T1, T2, T3, T4> FromRequest for (T1, T2, T3, T4)
where
    T1: FromRequest + Send,
    T2: FromRequest + Send,
    T3: FromRequest + Send,
    T4: FromRequest + Send,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3, t4) = futures::join!(
            T1::from_request(req),
            T2::from_request(req),
            T3::from_request(req),
            T4::from_request(req)
        );
        Ok((t1?, t2?, t3?, t4?))
    }
}

#[async_trait::async_trait]
impl<T1, T2, T3, T4, T5> FromRequest for (T1, T2, T3, T4, T5)
where
    T1: FromRequest + Send,
    T2: FromRequest + Send,
    T3: FromRequest + Send,
    T4: FromRequest + Send,
    T5: FromRequest + Send,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3, t4, t5) = futures::join!(
            T1::from_request(req),
            T2::from_request(req),
            T3::from_request(req),
            T4::from_request(req),
            T5::from_request(req)
        );
        Ok((t1?, t2?, t3?, t4?, t5?))
    }
}

#[doc(hidden)]
#[async_trait::async_trait]
pub trait Factory<S, E, T> {
    async fn call(&self, param: T) -> Result<S, E>;
}

#[doc(hidden)]
struct Handler<F, S, E, T>
where
    F: Factory<S, E, T>,
{
    hnd: F,
    _t: PhantomData<fn() -> (S, E, T)>,
}

impl<F, S, E, T> Handler<F, S, E, T>
where
    F: Factory<S, E, T>,
{
    fn new(hnd: F) -> Self {
        Handler { hnd, _t: PhantomData }
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E> Factory<S, E, ()> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn() -> I + Sync,
{
    async fn call(&self, _: ()) -> Result<S, E> {
        (self)().await
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T1> Factory<S, E, (T1,)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn(T1) -> I + Sync,
    T1: Send + 'static,
{
    async fn call(&self, param: (T1,)) -> Result<S, E> {
        (self)(param.0).await
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T1, T2> Factory<S, E, (T1, T2)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn(T1, T2) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
{
    async fn call(&self, param: (T1, T2)) -> Result<S, E> {
        (self)(param.0, param.1).await
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T1, T2, T3> Factory<S, E, (T1, T2, T3)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn(T1, T2, T3) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
    T3: Send + 'static,
{
    async fn call(&self, param: (T1, T2, T3)) -> Result<S, E> {
        (self)(param.0, param.1, param.2).await
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T1, T2, T3, T4> Factory<S, E, (T1, T2, T3, T4)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn(T1, T2, T3, T4) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
    T3: Send + 'static,
    T4: Send + 'static,
{
    async fn call(&self, param: (T1, T2, T3, T4)) -> Result<S, E> {
        (self)(param.0, param.1, param.2, param.3).await
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T1, T2, T3, T4, T5> Factory<S, E, (T1, T2, T3, T4, T5)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    FN: Fn(T1, T2, T3, T4, T5) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
    T3: Send + 'static,
    T4: Send + 'static,
    T5: Send + 'static,
{
    async fn call(&self, param: (T1, T2, T3, T4, T5)) -> Result<S, E> {
        (self)(param.0, param.1, param.2, param.3, param.4).await
    }
}

impl<F, S, E, T> From<Handler<F, S, E, T>> for BoxedHandler
where
    F: Factory<S, E, T> + 'static + Send + Sync,
    S: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    T: FromRequest + 'static + Send,
{
    fn from(t: Handler<F, S, E, T>) -> BoxedHandler {
        let hnd = Arc::new(t.hnd);

        let inner = move |req: RequestObjectWithData| {
            let hnd = Arc::clone(&hnd);
            Box::pin(async move {
                let out = {
                    let param = T::from_request(&req).await?;
                    hnd.call(param).await?
                };
                Ok(Box::new(out) as BoxedSerialize)
            })
                as std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
        };

        BoxedHandler(Box::new(inner))
    }
}

pub struct BoxedHandler(
    Box<
        dyn Fn(
                RequestObjectWithData,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
            + Send
            + Sync,
    >,
);

pub struct MapRouter(HashMap<String, BoxedHandler>);

impl Default for MapRouter {
    fn default() -> Self {
        MapRouter(HashMap::default())
    }
}

pub trait Router: Default {
    fn get(&self, name: &str) -> Option<&BoxedHandler>;
    fn insert(&mut self, name: String, handler: BoxedHandler) -> Option<BoxedHandler>;
}

impl Router for MapRouter {
    fn get(&self, name: &str) -> Option<&BoxedHandler> {
        self.0.get(name)
    }
    fn insert(&mut self, name: String, handler: BoxedHandler) -> Option<BoxedHandler> {
        self.0.insert(name, handler)
    }
}

/// Server/request handler
pub struct Server<R> {
    data: Arc<Extensions>,
    router: R,
}

/// Builder used to add methods to a server
///
/// Created with `Server::new` or `Server::with_state`
pub struct ServerBuilder<R> {
    data: Extensions,
    router: R,
}

impl Server<MapRouter> {
    pub fn new() -> ServerBuilder<MapRouter> {
        Server::with_router(MapRouter::default())
    }
}

impl<R: Router> Server<R> {
    pub fn with_router(router: R) -> ServerBuilder<R> {
        ServerBuilder { data: Extensions::new(), router }
    }
}

impl<R: Router> ServerBuilder<R> {
    /// Add a data/state storage container to the server
    pub fn with_data<T: Send + Sync + 'static>(mut self, data: Data<T>) -> Self {
        self.data.insert(data);
        self
    }

    /// Add a method handler to the server
    ///
    /// The method is an async function that takes up to 5 [`FromRequest`](trait.FromRequest.html) items
    /// and returns a value that can be resolved to a `TryFuture`, where `TryFuture::Ok` is a serializable object, e.g.:
    ///
    /// ```rust,no_run
    /// async fn handle(params: Params<(i32, String)>, data: Data<HashMap<String, String>>) -> Result<String, Error> { /* ... */ }
    /// ```
    pub fn with_method<N, S, E, F, T>(mut self, name: N, handler: F) -> Self
    where
        N: Into<String>,
        F: Factory<S, E, T> + Send + Sync + 'static,
        S: Serialize + Send + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest + Send + 'static,
    {
        self.router.insert(name.into(), Handler::new(handler).into());
        self
    }

    /// Convert the server builder into the finished struct, wrapped in an `Arc`
    pub fn finish(self) -> Arc<Server<R>> {
        let ServerBuilder { router, data } = self;
        Arc::new(Server { router, data: Arc::new(data) })
    }

    /// Convert the server builder into the finished struct
    pub fn finish_unwrapped(self) -> Server<R> {
        let ServerBuilder { router, data } = self;
        Server { router, data: Arc::new(data) }
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

impl<R> Server<R>
where
    R: Router + 'static,
{
    #[cfg(feature = "actix-web-v1-integration")]
    fn handle_bytes_compat(
        &self,
        bytes: Bytes,
    ) -> impl futures_v01::Future<Item = ResponseObjects, Error = ()> {
        self.handle_bytes(bytes).unit_error().boxed().compat()
    }

    /// Handle requests, and return appropriate responses
    pub fn handle<I: Into<RequestKind>>(&self, req: I) -> impl Future<Output = ResponseObjects> {
        match req.into() {
            RequestKind::Bytes(bytes) => future::Either::Left(self.handle_bytes(bytes)),
            RequestKind::RequestObject(req) => future::Either::Right(future::Either::Left(
                self.handle_request_object(req).map(From::from),
            )),
            RequestKind::ManyRequestObjects(reqs) => future::Either::Right(future::Either::Right(
                self.handle_many_request_objects(reqs).map(From::from),
            )),
        }
    }

    fn handle_request_object(
        &self,
        req: RequestObject,
    ) -> impl Future<Output = SingleResponseObject> {
        let req = RequestObjectWithData { inner: req, data: Arc::clone(&self.data) };

        let opt_id = match req.inner.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None,
        };

        if let Some(method) = self.router.get(req.inner.method.as_ref()) {
            let out = (&method.0)(req).then(|res| match res {
                Ok(val) => future::ready(SingleResponseObject::result(val, opt_id)),
                Err(e) => future::ready(SingleResponseObject::error(e, opt_id)),
            });
            future::Either::Left(out)
        } else {
            future::Either::Right(future::ready(SingleResponseObject::error(
                Error::METHOD_NOT_FOUND,
                opt_id,
            )))
        }
    }

    fn handle_many_request_objects<I: IntoIterator<Item = RequestObject>>(
        &self,
        reqs: I,
    ) -> impl Future<Output = ManyResponseObjects> {
        reqs.into_iter()
            .map(|r| self.handle_request_object(r))
            .collect::<futures::stream::FuturesUnordered<_>>()
            .filter_map(|res| async move {
                match res {
                    SingleResponseObject::One(r) => Some(r),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .map(|vec| {
                if vec.is_empty() {
                    ManyResponseObjects::Empty
                } else {
                    ManyResponseObjects::Many(vec)
                }
            })
    }

    fn handle_bytes(&self, bytes: Bytes) -> impl Future<Output = ResponseObjects> {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(bytes.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return future::Either::Left(future::ready(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        )));
                    }

                    let (okays, errs) = raw_reqs
                        .into_iter()
                        .map(|x| {
                            serde_json::from_str::<BytesRequestObject>(x.get())
                                .map(RequestObject::from)
                        })
                        .partition::<Vec<_>, _>(|x| x.is_ok());

                    let errs = errs
                        .into_iter()
                        .map(|_| ResponseObject::error(Error::INVALID_REQUEST, Id::Null))
                        .collect::<Vec<_>>();

                    future::Either::Right(future::Either::Left(
                        self.handle_many_request_objects(okays.into_iter().flat_map(|x| x)).map(
                            |res| match res {
                                ManyResponseObjects::Many(mut many) => {
                                    many.extend(errs);
                                    ResponseObjects::Many(many)
                                }
                                ManyResponseObjects::Empty => {
                                    if errs.is_empty() {
                                        ResponseObjects::Empty
                                    } else {
                                        ResponseObjects::Many(errs)
                                    }
                                }
                            },
                        ),
                    ))
                }
                OneOrManyRawValues::One(raw_req) => {
                    match serde_json::from_str::<BytesRequestObject>(raw_req.get())
                        .map(RequestObject::from)
                    {
                        Ok(rn) => future::Either::Right(future::Either::Right(
                            self.handle_request_object(rn).map(|res| match res {
                                SingleResponseObject::One(r) => ResponseObjects::One(r),
                                _ => ResponseObjects::Empty,
                            }),
                        )),
                        Err(_) => future::Either::Left(future::ready(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        ))),
                    }
                }
            }
        } else {
            future::Either::Left(future::ready(ResponseObjects::One(ResponseObject::error(
                Error::PARSE_ERROR,
                Id::Null,
            ))))
        }
    }

    #[cfg(feature = "actix-web-v1-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v04::NewService<
        Request = actix_web_v1::dev::ServiceRequest,
        Response = actix_web_v1::dev::ServiceResponse,
        Error = actix_web_v1::Error,
        Config = (),
        InitError = (),
    > {
        use futures_v01::{Future, Stream};

        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v1::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v1::Error::from)
                .fold(actix_web_v1::web::BytesMut::new(), move |mut body, chunk| {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v1::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes_compat(bytes.freeze()).then(|res| match res {
                        Ok(res_inner) => match res_inner {
                            ResponseObjects::Empty => Ok(actix_web_v1::dev::ServiceResponse::new(
                                req,
                                actix_web_v1::HttpResponse::NoContent().finish(),
                            )),
                            json => Ok(actix_web_v1::dev::ServiceResponse::new(
                                req,
                                actix_web_v1::HttpResponse::Ok().json(json),
                            )),
                        },
                        Err(_) => Ok(actix_web_v1::dev::ServiceResponse::new(
                            req,
                            actix_web_v1::HttpResponse::InternalServerError().into(),
                        )),
                    })
                });
            rt
        };

        actix_service_v04::service_fn::<_, _, _, ()>(inner)
    }

    #[cfg(feature = "actix-web-v2-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v2::dev::ServiceRequest,
        Response = actix_web_v2::dev::ServiceResponse,
        Error = actix_web_v2::Error,
        Config = (),
        InitError = (),
    > {
        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v2::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v2::Error::from)
                .try_fold(actix_web_v2::web::BytesMut::new(), move |mut body, chunk| async move {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v2::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes(bytes.freeze()).map(|res| match res {
                        ResponseObjects::Empty => Ok(actix_web_v2::dev::ServiceResponse::new(
                            req,
                            actix_web_v2::HttpResponse::NoContent().finish(),
                        )),
                        json => Ok(actix_web_v2::dev::ServiceResponse::new(
                            req,
                            actix_web_v2::HttpResponse::Ok().json(json),
                        )),
                    })
                });
            rt
        };

        actix_service_v1::fn_service::<_, _, _, _, _, _>(inner)
    }

    #[cfg(feature = "actix-web-v3-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v3::dev::ServiceRequest,
        Response = actix_web_v3::dev::ServiceResponse,
        Error = actix_web_v3::Error,
        Config = (),
        InitError = (),
    > {
        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v3::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v3::Error::from)
                .try_fold(actix_web_v3::web::BytesMut::new(), move |mut body, chunk| async move {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v3::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes(bytes.freeze()).map(|res| match res {
                        ResponseObjects::Empty => Ok(actix_web_v3::dev::ServiceResponse::new(
                            req,
                            actix_web_v3::HttpResponse::NoContent().finish(),
                        )),
                        json => Ok(actix_web_v3::dev::ServiceResponse::new(
                            req,
                            actix_web_v3::HttpResponse::Ok().json(json),
                        )),
                    })
                });
            rt
        };

        actix_service_v1::fn_service::<_, _, _, _, _, _>(inner)
    }

    #[cfg(feature = "hyper-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_hyper_web_service(self: Arc<Self>) -> Hyper<R> {
        Hyper(self)
    }

    #[cfg(all(
        feature = "actix-web-v1-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v2-integration"),
        not(feature = "actix-web-v3-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v04::NewService<
        Request = actix_web_v1::dev::ServiceRequest,
        Response = actix_web_v1::dev::ServiceResponse,
        Error = actix_web_v1::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    #[cfg(all(
        feature = "actix-web-v2-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v3-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v2::dev::ServiceRequest,
        Response = actix_web_v2::dev::ServiceResponse,
        Error = actix_web_v2::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    #[cfg(all(
        feature = "actix-web-v3-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v2-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v3::dev::ServiceRequest,
        Response = actix_web_v3::dev::ServiceResponse,
        Error = actix_web_v3::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    #[cfg(all(
        feature = "hyper-integration",
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v2-integration"),
        not(feature = "actix-web-v3-integration")
    ))]
    pub fn into_web_service(self: Arc<Self>) -> Hyper<R> {
        self.into_hyper_web_service()
    }
}

#[cfg(feature = "hyper-integration")]
pub struct Hyper<R>(pub(crate) Arc<Server<R>>);

#[cfg(feature = "hyper-integration")]
impl<R> hyper::service::Service<hyper::Request<hyper::Body>> for Hyper<R>
where
    R: Router + Send + Sync + 'static,
{
    type Response = hyper::Response<hyper::Body>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: hyper::Request<hyper::Body>) -> Self::Future {
        use hyper::body::HttpBody;

        let service = Arc::clone(&self.0);

        let rt = async move {
            let mut buf = if let Some(content_length) = req
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .and_then(|x| x.to_str().ok())
                .and_then(|x| x.parse().ok())
            {
                bytes_v10::BytesMut::with_capacity(content_length)
            } else {
                bytes_v10::BytesMut::default()
            };

            let mut body = req.into_body();

            while let Some(chunk) = body.data().await {
                buf.extend(chunk?);
            }

            match service.handle_bytes(buf.freeze()).await {
                ResponseObjects::Empty => hyper::Response::builder()
                    .status(hyper::StatusCode::NO_CONTENT)
                    .body(hyper::Body::from(Vec::<u8>::new()))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
                json => serde_json::to_vec(&json)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .and_then(|json| {
                        hyper::Response::builder()
                            .status(hyper::StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(hyper::Body::from(json))
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }),
            }
        };
        Box::pin(rt)
    }
}

#[cfg(feature = "hyper-integration")]
impl<'a, R> tower_service::Service<&'a hyper::server::conn::AddrStream> for Hyper<R> {
    type Response = Hyper<R>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future = future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: &'a hyper::server::conn::AddrStream) -> Self::Future {
        future::ready(Ok(Hyper(Arc::clone(&self.0))))
    }
}
