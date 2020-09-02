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

*/

use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};

use serde_json::{value::RawValue, Value};

use std::sync::Arc;

use futures::{
    future::{self, Future, FutureExt},
    stream::StreamExt,
};

use std::{collections::HashMap, marker::PhantomData};

#[cfg(not(feature = "bytes-v04"))]
use bytes::Bytes;

#[cfg(feature = "bytes-v04")]
use bytes_v04::Bytes;

use paperclip::v2::models::DefaultSchemaRaw;
use paperclip::v2::schema::Apiv2Schema;

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
    pub const INVALID_REQUEST: Self = Error::Provided {
        code: -32600,
        message: "Invalid Request",
    };
    pub const METHOD_NOT_FOUND: Self = Error::Provided {
        code: -32601,
        message: "Method not found",
    };
    pub const INVALID_PARAMS: Self = Error::Provided {
        code: -32602,
        message: "Invalid params",
    };
    pub const INTERNAL_ERROR: Self = Error::Provided {
        code: -32603,
        message: "Internal Error",
    };
    pub const PARSE_ERROR: Self = Error::Provided {
        code: -32700,
        message: "Parse error",
    };

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
        Error::Full {
            code: t.code(),
            message: t.message(),
            data: t.data(),
        }
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
        RequestBuilder {
            id,
            params,
            method: method.into(),
        }
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
        NotificationBuilder {
            params,
            method: method.into(),
        }
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
        let BytesRequestObject {
            jsonrpc,
            method,
            params,
            id,
        } = t;
        RequestObject {
            jsonrpc,
            method,
            params: params.map(InnerParams::Raw),
            id,
        }
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

/// [`FromRequest`](trait.FromRequest.html) wrapper for request params
///
/// Use a tuple to deserialize by-position params
/// and a map or deserializable struct for by-name params: e.g.
///
/// ```
#[derive(paperclip::actix::Apiv2Schema, Deserialize)]
pub struct Params<T>(pub T);

/// A trait to extract data from the request
#[async_trait::async_trait]
pub trait FromRequest: Sized {
    async fn from_request(req: &RequestObject) -> Result<Self, Error>;
}

#[async_trait::async_trait]
impl<T: DeserializeOwned> FromRequest for Params<T> {
    async fn from_request(req: &RequestObject) -> Result<Self, Error> {
        let res = match req.params {
            Some(InnerParams::Raw(ref value)) => serde_json::from_str(value.get()),
            Some(InnerParams::Value(ref value)) => serde_json::from_value(value.clone()),
            None => serde_json::from_value(Value::Null),
        };

        Ok(res.map(Params).map_err(|_| Error::INVALID_PARAMS)?)
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

#[doc(hidden)]
#[async_trait::async_trait]
pub trait Factory<S, E, T, M> {
    async fn call(&self, param: T, metadata: M) -> Result<S, E>;
}

#[doc(hidden)]
struct Handler<F, S, E, T, M>
where
    F: Factory<S, E, T, M>,
{
    hnd: F,
    _t: PhantomData<fn() -> (S, E, T, M)>,
}

impl<F, S, E, T, M> Handler<F, S, E, T, M>
where
    F: Factory<S, E, T, M>,
{
    fn new(hnd: F) -> Self {
        Handler {
            hnd,
            _t: PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<FN, I, S, E, T, M> Factory<S, E, T, M> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + Send + 'static,
    T: FromRequest + Send + 'static,
    FN: Fn(T, M) -> I + Sync,
    M: Metadata,
{
    async fn call(&self, param: T, meta: M) -> Result<S, E> {
        (self)(param, meta).await
    }
}

impl<F, S, E, T, M> From<Handler<F, S, E, T, M>> for BoxedHandler<M>
where
    F: Factory<S, E, T, M> + 'static + Send + Sync,
    S: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    M: Metadata,
    T: FromRequest + 'static + Send,
{
    fn from(t: Handler<F, S, E, T, M>) -> BoxedHandler<M> {
        let hnd = Arc::new(t.hnd);
        let inner = move |req: RequestObject, metadata: M| {
            let hnd = Arc::clone(&hnd);
            Box::pin(async move {
                let out = {
                    let param = T::from_request(&req).await?;

                    hnd.call(param, metadata).await?
                };
                Ok(Box::new(out) as BoxedSerialize)
            })
                as std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
        };

        BoxedHandler(Box::new(inner))
    }
}

type HandlerResult = std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>;

pub struct BoxedHandler<M: Metadata>(Box<dyn Fn(RequestObject, M) -> HandlerResult + Send + Sync>);

pub struct MapRouter<M: Metadata>(HashMap<String, Route<M>>);

pub struct Route<M: Metadata> {
    handler: BoxedHandler<M>,
    middlewares: Vec<Arc<dyn Middleware<M>>>,
}

impl<M: Metadata> Default for MapRouter<M> {
    fn default() -> Self {
        MapRouter(HashMap::default())
    }
}

pub trait Router<M: Metadata>: Default {
    fn get(&self, name: &str) -> Option<&Route<M>>;
    fn insert(&mut self, name: String, route: Route<M>) -> Option<Route<M>>;
}

impl<M: Metadata> Router<M> for MapRouter<M> {
    fn get(&self, name: &str) -> Option<&Route<M>> {
        self.0.get(name)
    }
    fn insert(&mut self, name: String, route: Route<M>) -> Option<Route<M>> {
        self.0.insert(name, route)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DocNotification {
    name: String,
    notification: DefaultSchemaRaw,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DocRoute {
    name: String,
    request: DefaultSchemaRaw,
    response: DefaultSchemaRaw,
}

/// Server/request handler
pub struct Server<M>
where
    M: Metadata,
{
    router: MapRouter<M>,
    middlewares: Vec<Arc<dyn Middleware<M>>>,
}

/// Builder used to add methods to a server
///
/// Created with `Server::new` or `Server::with_state`
pub struct ServerBuilder<M>
where
    M: Metadata,
{
    router: MapRouter<M>,
    routes: Vec<DocRoute>,
    notifications: Vec<DocNotification>,
    middlewares: Vec<Arc<dyn Middleware<M>>>,
}

impl<M: Metadata> Server<M> {
    pub fn new() -> ServerBuilder<M> {
        Self::with_router(MapRouter::default())
    }
    pub fn with_router(router: MapRouter<M>) -> ServerBuilder<M> {
        ServerBuilder {
            router,
            routes: Vec::default(),
            notifications: Vec::default(),
            middlewares: Vec::default(),
        }
    }
}

impl<M: Metadata> ServerBuilder<M> {
    /// Add a method handler to the server
    ///
    /// The method is an async function that takes up to 5 [`FromRequest`](trait.FromRequest.html) items
    /// and returns a value that can be resolved to a `TryFuture`, where `TryFuture::Ok` is a serializable object, e.g.:
    ///

    pub fn with_method<'de, N, S, E, T, F>(mut self, name: N, handler: F) -> Self
    where
        N: Into<String> + Clone,
        F: Factory<S, E, T, M> + Send + Sync + 'static,
        S: Serialize + Deserialize<'de> + Send + Apiv2Schema + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest + Deserialize<'de> + Send + Apiv2Schema + 'static,
    {
        self.with_method_middleware(name, handler, vec![])
    }

    pub fn with_method_middleware<'de, N, S, E, T, F>(
        mut self,
        name: N,
        handler: F,
        middlewares: Vec<Arc<dyn Middleware<M>>>,
    ) -> Self
    where
        N: Into<String> + Clone,
        F: Factory<S, E, T, M> + Send + Sync + 'static,
        S: Serialize + Deserialize<'de> + Send + Apiv2Schema + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest + Deserialize<'de> + Send + Apiv2Schema + 'static,
    {
        self.routes.push(DocRoute {
            name: name.clone().into(),
            request: T::raw_schema(),
            response: S::raw_schema(),
        });
        let route = Route {
            handler: Handler::new(handler).into(),
            middlewares,
        };
        self.router.insert(name.into(), route);
        self
    }

    /// Convert the server builder into the finished struct, wrapped in an `Arc`
    pub fn finish(self) -> Arc<Server<M>> {
        let builder = self.add_documentation_route();
        Arc::new(Server {
            router: builder.router,
            middlewares: builder.middlewares,
        })
    }

    fn add_documentation_route(mut self) -> Self {
        let spec_handler = SpecHandler {
            routes: self.routes.clone(),
            notifications: self.notifications.clone(),
        };
        let route = Route {
            handler: Handler::new(spec_handler).into(),
            middlewares: vec![],
        };
        self.router.insert("__docs__".into(), route);
        self
    }

    /// Convert the server builder into the finished struct
    pub fn finish_unwrapped(self) -> Server<M> {
        let ServerBuilder {
            router,
            routes: _,
            notifications: _,
            middlewares,
        } = self;
        Server {
            router,
            middlewares,
        }
    }

    pub fn with_notification<'de, N: Deserialize<'de> + Send + Apiv2Schema + 'static>(
        mut self,
        name: String,
    ) -> Self {
        self.notifications.push(DocNotification {
            notification: N::raw_schema(),
            name,
        });
        self
    }
}

/// The individual response object
#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseObject {
    Result {
        jsonrpc: V2,
        result: BoxedSerialize,
        id: Id,
    },
    Error {
        jsonrpc: V2,
        error: Error,
        id: Id,
    },
}

impl ResponseObject {
    fn result(result: BoxedSerialize, id: Id) -> Self {
        ResponseObject::Result {
            jsonrpc: V2,
            result,
            id,
        }
    }

    fn error(error: Error, id: Id) -> Self {
        ResponseObject::Error {
            jsonrpc: V2,
            error,
            id,
        }
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
        if opt_id.is_none() {
            log::info!("id for request is none");
        }
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
            Ok(OneOrManyRawValues::Many(serde_json::from_slice::<
                Vec<&RawValue>,
            >(slice)?))
        } else {
            Ok(OneOrManyRawValues::One(
                serde_json::from_slice::<&RawValue>(slice)?,
            ))
        }
    }
}

pub trait Metadata: Clone + Send + 'static {}

impl Metadata for () {}

impl<M> Server<M>
where
    M: Metadata,
{
    #[cfg(feature = "actix-web-v1-integration")]
    fn handle_bytes_compat(
        &self,
        bytes: Bytes,
        metadata: M,
    ) -> impl futures_v01::Future<Item = ResponseObjects, Error = ()> {
        self.handle_bytes(bytes, metadata)
            .unit_error()
            .boxed()
            .compat()
    }

    /// Handle requests, and return appropriate responses
    pub fn handle<I: Into<RequestKind>>(
        &self,
        req: I,
        metadata: M,
    ) -> impl Future<Output = ResponseObjects> + '_ {
        match req.into() {
            RequestKind::Bytes(bytes) => future::Either::Left(self.handle_bytes(bytes, metadata)),
            RequestKind::RequestObject(req) => future::Either::Right(future::Either::Left(
                self.handle_request_object(req, metadata).map(From::from),
            )),
            RequestKind::ManyRequestObjects(reqs) => future::Either::Right(future::Either::Right(
                self.handle_many_request_objects(reqs, metadata)
                    .map(From::from),
            )),
        }
    }

    fn handle_request_object(
        &self,
        req: RequestObject,
        metadata: M,
    ) -> impl Future<Output = SingleResponseObject> + '_ {
        let opt_id = match req.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None,
        };

        if let Some(route) = self.router.get(req.method.as_ref()) {
            // let middlewares = [&self.middlewares[..], &route.middlewares[..]].concat();
            let next = Next {
                endpoint: &route.handler,
                next_middleware: &route.middlewares,
            };

            let out = next.run(req, metadata).then(|res| match res {
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
        metadata: M,
    ) -> impl Future<Output = ManyResponseObjects> + '_ {
        reqs.into_iter()
            .map(|r| self.handle_request_object(r, metadata.clone()))
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

    fn handle_bytes(
        &self,
        bytes: Bytes,
        metadata: M,
    ) -> impl Future<Output = ResponseObjects> + '_ {
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
                        self.handle_many_request_objects(okays.into_iter().flatten(), metadata)
                            .map(|res| match res {
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
                            }),
                    ))
                }
                OneOrManyRawValues::One(raw_req) => {
                    match serde_json::from_str::<BytesRequestObject>(raw_req.get())
                        .map(RequestObject::from)
                    {
                        Ok(rn) => future::Either::Right(future::Either::Right(
                            self.handle_request_object(rn, metadata)
                                .map(|res| match res {
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
}

type DocType = Vec<DocRoute>;
type Notifications = Vec<DocNotification>;

#[derive(Clone)]
struct SpecHandler {
    routes: DocType,
    notifications: Notifications,
}

#[derive(paperclip::actix::Apiv2Schema, Serialize, Deserialize)]
struct DummyReq {}

#[async_trait::async_trait]
impl<M> Factory<(DocType, Notifications), Error, Params<DummyReq>, M> for SpecHandler
where
    M: Metadata,
{
    async fn call(&self, _: Params<DummyReq>, _: M) -> Result<(DocType, Notifications), Error> {
        Ok((self.routes.clone(), self.notifications.clone()))
    }
}

pub struct Next<'a, 'b, M: Metadata> {
    endpoint: &'b BoxedHandler<M>,
    next_middleware: &'a [Arc<dyn Middleware<M>>],
}

#[async_trait::async_trait]
pub trait Middleware<M: Metadata>: Send + Sync + 'static {
    async fn handle(
        &self,
        req: RequestObject,
        metadata: M,
        next: Next<'_, '_, M>,
    ) -> Result<BoxedSerialize, Error>;
}

impl<M: Metadata> Next<'_, '_, M> {
    async fn run(mut self, req: RequestObject, metadata: M) -> Result<BoxedSerialize, Error> {
        if let Some((current, next)) = self.next_middleware.split_first() {
            self.next_middleware = next;
            current.handle(req, metadata, self).await
        } else {
            (&self.endpoint.0)(req, metadata).await
        }
    }
}
