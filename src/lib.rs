/*!
A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides an integration for `actix-web` servers.

# Usage

```rust,no_run
use jsonrpc_v2::*;

#[derive(serde::Deserialize)]
struct TwoNums { a: usize, b: usize }

fn add(Params(params): Params<TwoNums>) -> Result<usize, Error> {
    Ok(params.a + params.b)
}

fn sub(Params(params): Params<(usize, usize)>) -> Result<usize, Error> {
    Ok(params.0 - params.1)
}

fn message(state: State<String>) -> Result<String, Error> {
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
    future::{self, Either, Future, IntoFuture},
    stream::{futures_unordered, Stream},
};

use std::{collections::HashMap, marker::PhantomData};

type BoxedSerialize = Box<erased_serde::Serialize + Send>;

#[doc(hidden)]
#[derive(Debug)]
pub struct MethodMissing;

impl std::fmt::Display for MethodMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        "Cannot build request object with missing method".fmt(f)
    }
}

impl std::error::Error for MethodMissing {}

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
pub trait FromRequest<S>: Sized {
    type Result: IntoFuture<Item = Self, Error = Error>;
    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result;
}

impl<S> FromRequest<S> for () {
    type Result = Result<Self, Error>;
    fn from_request(_: &RequestObjectWithState<S>) -> Self::Result {
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

impl<S> FromRequest<S> for State<S> {
    type Result = Result<Self, Error>;

    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result {
        Ok(State(Arc::clone(&req.state)))
    }
}

impl<S, T: DeserializeOwned> FromRequest<S> for Params<T> {
    type Result = Result<Self, Error>;
    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result {
        Ok(Self::from_request_inner(&req.inner)?)
    }
}

impl<S, T1> FromRequest<S> for (T1,)
where
    T1: FromRequest<S>,
    <<T1 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
{
    type Result = Box<Future<Item = (T1,), Error = Error>>;

    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result {
        let rt = T1::from_request(req).into_future().map(|x| (x,));
        Box::new(rt)
    }
}

impl<S, T1, T2> FromRequest<S> for (T1, T2)
where
    T1: FromRequest<S>,
    T2: FromRequest<S>,
    <<T1 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
    <<T2 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
{
    type Result = Box<Future<Item = (T1, T2), Error = Error>>;

    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result {
        let rt = T1::from_request(req).into_future().join(T2::from_request(req));
        Box::new(rt)
    }
}

impl<S, T1, T2, T3> FromRequest<S> for (T1, T2, T3)
where
    T1: FromRequest<S>,
    T2: FromRequest<S>,
    T3: FromRequest<S>,
    <<T1 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
    <<T2 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
    <<T3 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
{
    type Result = Box<Future<Item = (T1, T2, T3), Error = Error>>;

    fn from_request(req: &RequestObjectWithState<S>) -> Self::Result {
        let rt = Future::join3(
            T1::from_request(req).into_future(),
            T2::from_request(req),
            T3::from_request(req),
        );
        Box::new(rt)
    }
}

#[doc(hidden)]
pub trait Factory<S, I, R, E, T>: Clone
where
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
{
    fn call(&self, param: T) -> I;
}

#[doc(hidden)]
struct Handler<F, S, I, R, E, T>
where
    F: Factory<S, I, R, E, T>,
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
{
    hnd: F,
    _t: PhantomData<fn() -> (S, I, S, E, T)>,
}

impl<F, S, I, R, E, T> Handler<F, S, I, R, E, T>
where
    F: Factory<S, I, R, E, T>,
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
{
    fn new(hnd: F) -> Self {
        Handler { hnd, _t: PhantomData }
    }
}

impl<FN, S, I, R, E> Factory<S, I, R, E, ()> for FN
where
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
    FN: Fn() -> I + Clone,
{
    fn call(&self, _: ()) -> I {
        (self)()
    }
}

impl<FN, S, I, R, E, T1> Factory<S, I, R, E, (T1,)> for FN
where
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
    FN: Fn(T1) -> I + Clone,
{
    fn call(&self, param: (T1,)) -> I {
        (self)(param.0)
    }
}

impl<FN, S, I, R, E, T1, T2> Factory<S, I, R, E, (T1, T2)> for FN
where
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
    FN: Fn(T1, T2) -> I + Clone,
{
    fn call(&self, param: (T1, T2)) -> I {
        (self)(param.0, param.1)
    }
}

impl<FN, S, I, R, E, T1, T2, T3> Factory<S, I,R, E, (T1, T2, T3)> for FN
where
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
    FN: Fn(T1, T2, T3) -> I + Clone,
{
    fn call(&self, param: (T1, T2, T3)) -> I {
        (self)(param.0, param.1, param.2)
    }
}

impl<F, S, I, R, E, T> From<Handler<F, S, I, R, E, T>> for BoxedHandler<S>
where
    F: Factory<S, I, R, E, T> + 'static + Send + Sync,
    S: 'static,
    I: IntoFuture<Item = R, Error = E> + 'static,
    R: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    T: FromRequest<S> + 'static,
    <<T as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
{
    fn from(t: Handler<F, S, I, R, E, T>) -> BoxedHandler<S> {
        let arc = Arc::new(t.hnd);

        let inner = move |req: RequestObjectWithState<S>| {
            let cloned = Arc::clone(&arc);
            let rt = T::from_request(&req)
                .into_future()
                .and_then(move |param| cloned.call(param).into_future().map_err(Error::from))
                .map(|s| Box::new(s) as BoxedSerialize);
            Box::new(rt) as Box<Future<Item = BoxedSerialize, Error = Error>>
        };

        BoxedHandler(Box::new(inner))
    }
}

struct BoxedHandler<S>(
    Box<
        Fn(RequestObjectWithState<S>) -> Box<Future<Item = BoxedSerialize, Error = Error>>
            + Send
            + Sync,
    >,
);

/// Server/request handler
pub struct Server<S>(Arc<ServerBuilder<S>>);

impl<S> Clone for Server<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

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
    pub fn with_method<N, I, R, E, F, T>(mut self, name: N, handler: F) -> Self
    where
        N: Into<String>,
        F: Factory<S, I, R, E, T> + Send + Sync + 'static,
        I: IntoFuture<Item = R, Error = E> + 'static,
        R: Serialize + Send + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest<S> + 'static,
        <<T as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
    {
        self.methods.insert(name.into(), Handler::new(handler).into());
        self
    }

    /// Convert the server builder into the finished struct
    pub fn finish(self) -> Server<S> {
        Server(Arc::new(self))
    }
}

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
    /// Handle requests, and return appropriate responses
    pub fn handle<I: Into<RequestKind>>(
        &self,
        req: I,
    ) -> impl Future<Item = ResponseObjects, Error = ()> {
        match req.into() {
            RequestKind::Bytes(bytes) => Either::A(self.handle_bytes(bytes)),
            RequestKind::RequestObject(req) => {
                Either::B(Either::A(self.handle_request_object(req).map(From::from)))
            }
            RequestKind::ManyRequestObjects(reqs) => {
                Either::B(Either::B(self.handle_many_request_objects(reqs).map(From::from)))
            }
        }
    }

    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_web_service(
        self,
    ) -> impl actix_service::NewService<
        Request = actix_web::dev::ServiceRequest,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        Config = (),
        InitError = (),
    > {
        let inner = move |req: actix_web::dev::ServiceRequest| {
            let cloned = self.clone();
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web::Error::from)
                .fold(actix_web::web::BytesMut::new(), move |mut body, chunk| {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web::Error>(body)
                })
                .and_then(move |bytes| {
                    cloned.handle_bytes(bytes.freeze()).then(|res| match res {
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

    fn handle_request_object(
        &self,
        req: RequestObject,
    ) -> impl Future<Item = SingleResponseObject, Error = ()> {
        let req = RequestObjectWithState { inner: req, state: Arc::clone(&self.0.state) };

        let opt_id = match req.inner.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None,
        };

        let rt = if let Some(method) = self.0.methods.get(req.inner.method.as_ref()) {
            let rt = (&method.0)(req).then(|fut| match fut {
                Ok(val) => future::ok(SingleResponseObject::result(val, opt_id)),
                Err(e) => future::ok(SingleResponseObject::error(e, opt_id)),
            });
            Either::A(rt)
        } else {
            let rt = future::ok(SingleResponseObject::error(Error::METHOD_NOT_FOUND, opt_id));
            Either::B(rt)
        };

        rt
    }

    fn handle_many_request_objects<I: IntoIterator<Item = RequestObject>>(
        &self,
        reqs: I,
    ) -> impl Future<Item = ManyResponseObjects, Error = ()> {
        futures_unordered(reqs.into_iter().map(|r| self.handle_request_object(r)))
            .filter_map(|res| match res {
                SingleResponseObject::One(r) => Some(r),
                _ => None,
            })
            .collect()
            .map(|vec| {
                if vec.is_empty() {
                    ManyResponseObjects::Empty
                } else {
                    ManyResponseObjects::Many(vec)
                }
            })
    }

    fn handle_bytes(&self, bytes: bytes::Bytes) -> impl Future<Item = ResponseObjects, Error = ()> {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(bytes.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return Either::A(Either::A(future::ok(ResponseObjects::One(
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

                    return Either::A(Either::B(
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
                    ));
                }
                OneOrManyRawValues::One(raw_req) => {
                    return Either::B(match serde_json::from_str::<RequestObject>(raw_req.get()) {
                        Ok(rn) => Either::A(self.handle_request_object(rn).map(|res| match res {
                            SingleResponseObject::One(r) => ResponseObjects::One(r),
                            _ => ResponseObjects::Empty,
                        })),
                        Err(_) => Either::B(future::ok(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        ))),
                    });
                }
            }
        }

        Either::A(Either::A(future::ok(ResponseObjects::One(ResponseObject::error(
            Error::PARSE_ERROR,
            Id::Null,
        )))))
    }
}
