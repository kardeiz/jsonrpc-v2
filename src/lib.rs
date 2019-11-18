/*!
A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides integrations for both `hyper` and `actix-web`. Enable features `actix` or `hyper` depending on need.

`actix` is enabled by default. Make sure to add `default-features = false` if using `hyper`.

Also see the `easy-errors` feature flag (not enabled by default). Enabling this flag will implement [`ErrorLike`](trait.ErrorLike.html)
for anything that implements `Display`, and the display value will be provided in the `message` field of the JSON-RPC 2.0 `Error` response.

Otherwise, custom errors should implement [`ErrorLike`](trait.ErrorLike.html) to map errors to the JSON-RPC 2.0 `Error` response.

Individual method handlers are `async` functions that can take various kinds of args (things that can be extracted from the request, like
the `Params` or `State`), and should return a `Result<Item, Error>` where the `Item` is serializable. See examples below.

# Usage

```rust,no_run
use jsonrpc_v2::*;

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

async fn message(state: State<String>) -> Result<String, Error> {
    Ok(String::from(&*state))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::with_state(String::from("Hello!"))
        .with_method("add", add)
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
    .run()?;

    Ok(())
}
```
*/

use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};

use serde_json::{value::RawValue, Value};

use std::sync::Arc;

use futures::{
    future::{self, Future, FutureExt, TryFutureExt},
    stream::TryStreamExt,
};

use std::{collections::HashMap, marker::PhantomData};

type BoxedSerialize = Box<dyn erased_serde::Serialize + Send>;

#[doc(hidden)]
#[derive(Debug)]
pub struct MethodMissing;

impl std::fmt::Display for MethodMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        "Cannot build request object with missing method".fmt(f)
    }
}

impl std::error::Error for MethodMissing {}

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
    pub const INVALID_PARAMS: Self = Error::Provided { code: -32602, message: "Invalid params" };
    pub const INVALID_REQUEST: Self = Error::Provided { code: -32600, message: "Invalid Request" };
    pub const METHOD_NOT_FOUND: Self =
        Error::Provided { code: -32601, message: "Method not found" };
    pub const PARSE_ERROR: Self = Error::Provided { code: -32700, message: "Parse error" };
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

#[cfg(feature = "easy_errors")]
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

/// Builder struct for a request object
#[derive(Default)]
pub struct RequestBuilder {
    method: Option<String>,
    params: Option<Value>,
    id: Id,
}

/// Builder struct for a notification request object
#[derive(Default)]
pub struct NotificationBuilder {
    method: Option<String>,
    params: Option<Value>,
}

impl RequestBuilder {
    pub fn with_id<I: Into<Id>>(mut self, id: I) -> Self {
        self.id = id.into();
        self
    }
    pub fn with_method<I: Into<String>>(mut self, method: I) -> Self {
        self.method = Some(method.into());
        self
    }
    pub fn with_params<I: Into<Value>>(mut self, params: I) -> Self {
        self.params = Some(params.into());
        self
    }
    pub fn finish(self) -> Result<RequestObject, MethodMissing> {
        let RequestBuilder { method, params, id } = self;
        match method {
            Some(method) => Ok(RequestObject {
                jsonrpc: V2,
                method: method.into_boxed_str(),
                params,
                id: Some(Some(id)),
            }),
            None => Err(MethodMissing),
        }
    }
}

impl NotificationBuilder {
    pub fn with_method<I: Into<String>>(mut self, method: I) -> Self {
        self.method = Some(method.into());
        self
    }
    pub fn with_params<I: Into<Value>>(mut self, params: I) -> Self {
        self.params = Some(params.into());
        self
    }
    pub fn finish(self) -> Result<RequestObject, MethodMissing> {
        let NotificationBuilder { method, params } = self;
        match method {
            Some(method) => {
                Ok(RequestObject { jsonrpc: V2, method: method.into_boxed_str(), params, id: None })
            }
            None => Err(MethodMissing),
        }
    }
}

/// Request/Notification object
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RequestObject {
    jsonrpc: V2,
    method: Box<str>,
    params: Option<Value>,
    #[serde(deserialize_with = "RequestObject::deserialize_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Option<Id>>,
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
pub struct RequestObjectWithState<S> {
    inner: RequestObject,
    state: Arc<S>,
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
    fn from_request_inner(req: &RequestObject) -> Result<Self, Error> {
        let res = match req.params {
            Some(ref value) => serde_json::from_value(value.clone()),
            None => serde_json::from_value(Value::Null),
        };
        res.map(Params).map_err(|_| Error::INVALID_PARAMS)
    }
}

/// A trait to extract data from the request
#[async_trait::async_trait]
pub trait FromRequest<S>: Sized {
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error>;
}

#[async_trait::async_trait]
impl<S: Send + Sync> FromRequest<S> for () {
    async fn from_request(_: &RequestObjectWithState<S>) -> Result<Self, Error> {
        Ok(())
    }
}

/// A wrapper around the (optional) state object provided when creating the server
pub struct State<S>(Arc<S>);

impl<S> std::ops::Deref for State<S> {
    type Target = S;

    fn deref(&self) -> &S {
        &*self.0
    }
}

#[async_trait::async_trait]
impl<S: Send + Sync> FromRequest<S> for State<S> {
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error> {
        Ok(State(Arc::clone(&req.state)))
    }
}

#[async_trait::async_trait]
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Params<T> {
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error> {
        Ok(Self::from_request_inner(&req.inner)?)
    }
}

#[async_trait::async_trait]
impl<S: Send + Sync, T1> FromRequest<S> for (T1,)
where
    T1: FromRequest<S> + Send,
{
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error> {
        Ok((T1::from_request(req).await?,))
    }
}

#[async_trait::async_trait]
impl<S: Send + Sync, T1, T2> FromRequest<S> for (T1, T2)
where
    T1: FromRequest<S> + Send,
    T2: FromRequest<S> + Send,
{
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error> {
        let (t1, t2) = futures::join!(T1::from_request(req), T2::from_request(req));
        Ok((t1?, t2?))
    }
}

#[async_trait::async_trait]
impl<S: Send + Sync, T1, T2, T3> FromRequest<S> for (T1, T2, T3)
where
    T1: FromRequest<S> + Send,
    T2: FromRequest<S> + Send,
    T3: FromRequest<S> + Send,
{
    async fn from_request(req: &RequestObjectWithState<S>) -> Result<Self, Error> {
        let (t1, t2, t3) =
            futures::join!(T1::from_request(req), T2::from_request(req), T3::from_request(req));
        Ok((t1?, t2?, t3?))
    }
}

#[doc(hidden)]
#[async_trait::async_trait]
pub trait Factory<S, R, E, T>: Clone
where
    S: 'static,
{
    async fn call(&self, param: T) -> Result<R, E>;
}

#[doc(hidden)]
struct Handler<F, S, R, E, T>
where
    F: Factory<S, R, E, T>,
    S: 'static,
{
    hnd: F,
    _t: PhantomData<fn() -> (S, R, E, T)>,
}

impl<F, S, R, E, T> Handler<F, S, R, E, T>
where
    F: Factory<S, R, E, T>,
    S: 'static,
{
    fn new(hnd: F) -> Self {
        Handler { hnd, _t: PhantomData }
    }
}

#[async_trait::async_trait]
impl<FN, S, I, R, E> Factory<S, R, E, ()> for FN
where
    S: 'static,
    R: 'static,
    E: 'static,
    I: Future<Output = Result<R, E>> + Send + 'static,
    FN: Fn() -> I + Clone + Sync,
{
    async fn call(&self, _: ()) -> Result<R, E> {
        (self)().await
    }
}

#[async_trait::async_trait]
impl<FN, S, I, R, E, T1> Factory<S, R, E, (T1,)> for FN
where
    S: 'static,
    R: 'static,
    E: 'static,
    I: Future<Output = Result<R, E>> + Send + 'static,
    FN: Fn(T1) -> I + Clone + Sync,
    T1: Send + 'static,
{
    async fn call(&self, param: (T1,)) -> Result<R, E> {
        (self)(param.0).await
    }
}

#[async_trait::async_trait]
impl<FN, S, I, R, E, T1, T2> Factory<S, R, E, (T1, T2)> for FN
where
    S: 'static,
    R: 'static,
    E: 'static,
    I: Future<Output = Result<R, E>> + Send + 'static,
    FN: Fn(T1, T2) -> I + Clone + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
{
    async fn call(&self, param: (T1, T2)) -> Result<R, E> {
        (self)(param.0, param.1).await
    }
}

#[async_trait::async_trait]
impl<FN, S, I, R, E, T1, T2, T3> Factory<S, R, E, (T1, T2, T3)> for FN
where
    S: 'static,
    R: 'static,
    E: 'static,
    I: Future<Output = Result<R, E>> + Send + 'static,
    FN: Fn(T1, T2, T3) -> I + Clone + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
    T3: Send + 'static,
{
    async fn call(&self, param: (T1, T2, T3)) -> Result<R, E> {
        (self)(param.0, param.1, param.2).await
    }
}

impl<F, S, R, E, T> From<Handler<F, S, R, E, T>> for BoxedHandler<S>
where
    F: Factory<S, R, E, T> + 'static + Send + Sync,
    S: 'static + Send + Sync,
    R: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    T: FromRequest<S> + 'static + Send,
{
    fn from(t: Handler<F, S, R, E, T>) -> BoxedHandler<S> {
        let arc = Arc::new(t.hnd);

        let inner = move |req: RequestObjectWithState<S>| {
            let cloned = Arc::clone(&arc);
            Box::pin(async move {
                let out = {
                    let param = T::from_request(&req).await?;
                    cloned.call(param).await?
                };
                Ok(Box::new(out) as BoxedSerialize)
            })
                as std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
        };

        BoxedHandler(Box::new(inner))
    }
}

struct BoxedHandler<S>(
    Box<
        dyn Fn(
                RequestObjectWithState<S>,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
            + Send
            + Sync,
    >,
);

/// Server/request handler
pub struct Server<S>(ServerBuilder<S>);

/// Builder used to add methods to a server
///
/// Created with `Server::new` or `Server::with_state`
pub struct ServerBuilder<S> {
    state: Arc<S>,
    methods: HashMap<String, BoxedHandler<S>>,
}

impl Server<()> {
    /// Create a new server with empty state (`()`)
    pub fn new() -> ServerBuilder<()> {
        ServerBuilder { state: Arc::new(()), methods: HashMap::new() }
    }
}

impl<S: 'static + Send + Sync> Server<S> {
    /// Create a new server with the given state
    pub fn with_state(state: S) -> ServerBuilder<S> {
        ServerBuilder { state: Arc::new(state), methods: HashMap::new() }
    }
}

impl<S: 'static + Send + Sync> ServerBuilder<S> {
    /// Add a method and handler to the server
    ///
    /// The method can be a function that takes up to 3 [`FromRequest`](trait.FromRequest.html) items
    /// and returns a value that can be resolved to a future of a serializable object, e.g.:
    ///
    /// ```rust,no_run
    /// fn handle(params: Params<(i32, String)>, state: State<HashMap<String, String>>) -> Result<String, Error> { /* ... */ }
    /// ```
    pub fn with_method<N, R, E, F, T>(mut self, name: N, handler: F) -> Self
    where
        N: Into<String>,
        F: Factory<S, R, E, T> + Send + Sync + 'static,
        R: Serialize + Send + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest<S> + Send + 'static,
    {
        self.methods.insert(name.into(), Handler::new(handler).into());
        self
    }

    /// Convert the server builder into the finished struct, wrapped in an `Arc`
    pub fn finish(self) -> Arc<Server<S>> {
        Arc::new(Server(self))
    }

    /// Convert the server builder into the finished struct
    pub fn finish_direct(self) -> Server<S> {
        Server(self)
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
    Bytes(bytes::Bytes),
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

impl From<bytes::Bytes> for RequestKind {
    fn from(t: bytes::Bytes) -> Self {
        RequestKind::Bytes(t)
    }
}

impl<'a> From<&'a [u8]> for RequestKind {
    fn from(t: &'a [u8]) -> Self {
        bytes::Bytes::from(t).into()
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

impl<S> Server<S>
where
    S: 'static,
{
    fn handle_bytes_compat(
        &self,
        bytes: bytes::Bytes,
    ) -> impl futures01::Future<Item = ResponseObjects, Error = ()> {
        self.handle_bytes(bytes).boxed().compat()
    }

    /// Handle requests, and return appropriate responses
    pub fn handle<I: Into<RequestKind>>(
        &self,
        req: I,
    ) -> impl Future<Output = Result<ResponseObjects, ()>> {
        match req.into() {
            RequestKind::Bytes(bytes) => future::Either::Left(self.handle_bytes(bytes)),
            RequestKind::RequestObject(req) => future::Either::Right(future::Either::Left(
                self.handle_request_object(req).map_ok(From::from),
            )),
            RequestKind::ManyRequestObjects(reqs) => future::Either::Right(future::Either::Right(
                self.handle_many_request_objects(reqs).map_ok(From::from),
            )),
        }
    }

    fn handle_request_object(
        &self,
        req: RequestObject,
    ) -> impl Future<Output = Result<SingleResponseObject, ()>> {
        let req = RequestObjectWithState { inner: req, state: Arc::clone(&self.0.state) };

        let opt_id = match req.inner.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None,
        };

        if let Some(method) = self.0.methods.get(req.inner.method.as_ref()) {
            let out = (&method.0)(req).then(|res| match res {
                Ok(val) => future::ready(Ok(SingleResponseObject::result(val, opt_id))),
                Err(e) => future::ready(Ok(SingleResponseObject::error(e, opt_id))),
            });
            future::Either::Left(out)
        } else {
            future::Either::Right(future::ready(Ok(SingleResponseObject::error(
                Error::METHOD_NOT_FOUND,
                opt_id,
            ))))
        }
    }

    fn handle_many_request_objects<I: IntoIterator<Item = RequestObject>>(
        &self,
        reqs: I,
    ) -> impl Future<Output = Result<ManyResponseObjects, ()>> {
        reqs.into_iter()
            .map(|r| self.handle_request_object(r))
            .collect::<futures::stream::FuturesUnordered<_>>()
            .try_filter_map(|res| {
                async move {
                    match res {
                        SingleResponseObject::One(r) => Ok(Some(r)),
                        _ => Ok(None),
                    }
                }
            })
            .try_collect::<Vec<_>>()
            .map_ok(|vec| {
                if vec.is_empty() {
                    ManyResponseObjects::Empty
                } else {
                    ManyResponseObjects::Many(vec)
                }
            })
    }

    fn handle_bytes(
        &self,
        bytes: bytes::Bytes,
    ) -> impl Future<Output = Result<ResponseObjects, ()>> {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(bytes.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return future::Either::Left(future::ready(Ok(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        ))));
                    }

                    let (okays, errs) = raw_reqs
                        .into_iter()
                        .map(|x| serde_json::from_str::<RequestObject>(x.get()))
                        .partition::<Vec<_>, _>(|x| x.is_ok());

                    let errs = errs
                        .into_iter()
                        .map(|_| ResponseObject::error(Error::INVALID_REQUEST, Id::Null))
                        .collect::<Vec<_>>();

                    future::Either::Right(future::Either::Left(
                        self.handle_many_request_objects(okays.into_iter().flat_map(|x| x)).map_ok(
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
                    match serde_json::from_str::<RequestObject>(raw_req.get()) {
                        Ok(rn) => future::Either::Right(future::Either::Right(
                            self.handle_request_object(rn).map_ok(|res| match res {
                                SingleResponseObject::One(r) => ResponseObjects::One(r),
                                _ => ResponseObjects::Empty,
                            }),
                        )),
                        Err(_) => future::Either::Left(future::ready(Ok(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        )))),
                    }
                }
            }
        } else {
            future::Either::Left(future::ready(Ok(ResponseObjects::One(ResponseObject::error(
                Error::PARSE_ERROR,
                Id::Null,
            )))))
        }
    }

    #[cfg(feature = "actix-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service::NewService<
        Request = actix_web::dev::ServiceRequest,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        Config = (),
        InitError = (),
    > {
        use futures01::{Future, Stream};

        let cloned = Arc::new(self);

        let inner = move |req: actix_web::dev::ServiceRequest| {
            let cloned = cloned.clone();
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web::Error::from)
                .fold(actix_web::web::BytesMut::new(), move |mut body, chunk| {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web::Error>(body)
                })
                .and_then(move |bytes| {
                    cloned.handle_bytes_compat(bytes.freeze()).then(|res| match res {
                        Ok(res_inner) => match res_inner {
                            ResponseObjects::Empty => Ok(actix_web::dev::ServiceResponse::new(
                                req,
                                actix_web::HttpResponse::NoContent().finish(),
                            )),
                            json => Ok(actix_web::dev::ServiceResponse::new(
                                req,
                                actix_web::HttpResponse::Ok().json(json),
                            )),
                        },
                        Err(_) => Ok(actix_web::dev::ServiceResponse::new(
                            req,
                            actix_web::HttpResponse::InternalServerError().into(),
                        )),
                    })
                });
            rt
        };

        actix_service::service_fn::<_, _, _, ()>(inner)
    }

    #[cfg(feature = "hyper-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_hyper_web_service(self: Arc<Self>) -> Hyper<S> {
        Hyper(self)
    }

    #[cfg(all(feature = "actix-integration", not(feature = "hyper-integration")))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service::NewService<
        Request = actix_web::dev::ServiceRequest,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    #[cfg(all(feature = "hyper-integration", not(feature = "actix-integration")))]
    pub fn into_web_service(self) -> Hyper<S> {
        self.into_hyper_web_service()
    }
}

#[cfg(feature = "hyper-integration")]
pub struct Hyper<S>(pub(crate) Arc<Server<S>>);

#[cfg(feature = "hyper-integration")]
impl<S> tower_service::Service<hyper::Request<hyper::Body>> for Hyper<S>
where
    S: 'static + Send + Sync,
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
        let service = Arc::clone(&self.0);

        let rt = async move {
            let mut buf = hyper::Chunk::default();
            let mut body = req.into_body();

            while let Some(chunk) = body.next().await {
                buf.extend(chunk?);
            }

            match service.handle_bytes(buf.into_bytes()).await {
                Ok(res_inner) => match res_inner {
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
                                .map_err(|e| {
                                    Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                                })
                        }),
                },
                Err(_) => hyper::Response::builder()
                    .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
                    .body(hyper::Body::from(Vec::<u8>::new()))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        };
        Box::pin(rt)
    }
}

#[cfg(feature = "hyper-integration")]
impl<'a, S> tower_service::Service<&'a hyper::server::conn::AddrStream> for Hyper<S>
where
    S: 'static + Send + Sync,
{
    type Response = Hyper<S>;
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
