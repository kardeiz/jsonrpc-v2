use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};

use serde_json::{value::RawValue, Value};

use std::sync::Arc;

use futures::{
    future::{ok as future_ok, Either as EitherFuture, Future, IntoFuture},
    stream::{futures_unordered, Stream}
};

use std::{collections::HashMap, marker::PhantomData};

use actix::prelude::*;

type BoxedSerialize = Box<erased_serde::Serialize + Send>;

#[derive(Default, Debug)]
pub struct V2;

impl Serialize for V2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        "2.0".serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for V2 {
    fn deserialize<D>(deserializer: D) -> Result<V2, D::Error>
    where D: Deserializer<'de> {
        let s: &str = Deserialize::deserialize(deserializer)?;
        if s == "2.0" {
            Ok(V2)
        } else {
            Err(serde::de::Error::custom("Could not deserialize V2"))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Num(i64),
    Str(Box<str>),
    Null
}

impl From<i64> for Id {
    fn from(t: i64) -> Self { Id::Num(t) }
}

impl<'a> From<&'a str> for Id {
    fn from(t: &'a str) -> Self { Id::Str(t.into()) }
}

impl From<String> for Id {
    fn from(t: String) -> Self { Id::Str(t.into()) }
}

impl Default for Id {
    fn default() -> Self { Id::Null }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RequestObject {
    jsonrpc: V2,
    method: Box<str>,
    params: Option<Box<RawValue>>,
    #[serde(deserialize_with = "RequestObject::deserialize_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Option<Id>>
}

impl RequestObject {
    fn deserialize_id<'de, D>(deserializer: D) -> Result<Option<Option<Id>>, D::Error>
    where D: Deserializer<'de> {
        Ok(Some(Option::deserialize(deserializer)?))
    }
}

pub struct WrappedRequestObject<S> {
    inner: RequestObject,
    state: Arc<S>
}

#[derive(Deserialize)]
pub struct Params<T>(pub T);

impl<T> Params<T>
where T: DeserializeOwned
{
    fn from_request_inner(req: &RequestObject) -> Result<Self, Error> {
        let res = match req.params {
            Some(ref raw_value) => serde_json::from_str(raw_value.get()),
            None => serde_json::from_value(Value::Null)
        };
        res.map(Params).map_err(|_| Error::INVALID_PARAMS)
    }
}

pub trait FromRequest<S>: Sized {
    type Result: IntoFuture<Item = Self, Error = Error>;
    fn from_request(req: &WrappedRequestObject<S>) -> Self::Result;
}

impl<S> FromRequest<S> for () {
    type Result = Result<Self, Error>;

    fn from_request(_: &WrappedRequestObject<S>) -> Self::Result { Ok(()) }
}

pub struct State<S>(Arc<S>);

impl<S> std::ops::Deref for State<S> {
    type Target = S;

    fn deref(&self) -> &S { &*self.0 }
}

impl<S> FromRequest<S> for State<S> {
    type Result = Result<Self, Error>;

    fn from_request(req: &WrappedRequestObject<S>) -> Self::Result {
        Ok(State(Arc::clone(&req.state)))
    }
}

impl<S, T1, T2> FromRequest<S> for (T1, T2) where 
    T1: FromRequest<S>, 
    T2: FromRequest<S>,
    <<T1 as FromRequest<S>>::Result as IntoFuture>::Future: 'static,
    <<T2 as FromRequest<S>>::Result as IntoFuture>::Future: 'static
    {
    type Result = Box<Future<Item=(T1, T2), Error=Error>>;

    fn from_request(req: &WrappedRequestObject<S>) -> Self::Result {
        let rt = T1::from_request(req).into_future()
            .join(T2::from_request(req));
        Box::new(rt)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Error {
    Full {
        code: i64,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<BoxedSerialize>
    },
    PreDef {
        code: i64,
        message: &'static str
    }
}

impl Error {
    pub const INVALID_PARAMS: Self = Error::PreDef { code: -32602, message: "Invalid params" };
    pub const INVALID_REQUEST: Self = Error::PreDef { code: -32600, message: "Invalid Request" };
    pub const METHOD_NOT_FOUND: Self = Error::PreDef { code: -32601, message: "Method not found" };
    pub const PARSE_ERROR: Self = Error::PreDef { code: -32700, message: "Parse error" };
}

pub trait ErrorLike {
    fn code(&self) -> i64;
    fn message(&self) -> String;
    fn data(&self) -> Option<BoxedSerialize> { None }
}

impl<T> From<T> for Error
where T: ErrorLike
{
    fn from(t: T) -> Error { Error::Full { code: t.code(), message: t.message(), data: t.data() } }
}

#[cfg(feature = "easy_errors")]
impl<T> ErrorLike for T
where T: std::fmt::Display
{
    fn code(&self) -> i64 { 0 }

    fn message(&self) -> String { self.to_string() }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Response {
    Result { jsonrpc: V2, result: BoxedSerialize, id: Id },
    Error { jsonrpc: V2, error: Error, id: Id }
}

impl Response {
    fn result(result: BoxedSerialize, id: Id) -> Self {
        Response::Result { jsonrpc: V2, result, id }
    }

    fn error(error: Error, id: Id) -> Self { Response::Error { jsonrpc: V2, error, id } }
}

pub struct With<T, S, P> {
    handler: Arc<
        Fn(Params<P>, T) -> Box<Future<Item = BoxedSerialize, Error = Error>>
            + 'static
            + Send
            + Sync
    >,
    _s: PhantomData<S>
}

impl<S, P, FN, I, E, FS, T> From<FN> for With<T, S, P>
where
    T: FromRequest<S> + 'static,
    S: 'static,
    P: DeserializeOwned,
    FN: Fn(Params<P>, T) -> I + 'static + Send + Sync,
    I: IntoFuture<Item = FS, Error = E> + 'static,
    I::Future: 'static,
    FS: Serialize + Send + 'static,
    E: Into<Error>
{
    fn from(u: FN) -> Self {
        let handler = move |params, t| {
            let rt = (u)(params, t)
                .into_future()
                .map_err(|e| e.into())
                .map(|res| Box::new(res) as BoxedSerialize);

            Box::new(rt) as Box<Future<Item = BoxedSerialize, Error = Error>>
        };
        With { handler: Arc::new(handler), _s: PhantomData }
    }
}

trait Method<S>: 'static + Send + Sync {
    fn handle(
        &self,
        req: &WrappedRequestObject<S>
    ) -> Box<Future<Item = BoxedSerialize, Error = Error>>;
}

impl<T, S, P> Method<S> for With<T, S, P>
where
    T: FromRequest<S> + 'static,
    P: DeserializeOwned,
    P: 'static + Send + Sync,
    S: 'static + Send + Sync
{
    fn handle(
        &self,
        req: &WrappedRequestObject<S>
    ) -> Box<Future<Item = BoxedSerialize, Error = Error>>
    {
        let handler = Arc::clone(&self.handler);
        let fut = Params::from_request_inner(&req.inner)
            .into_future()
            .join(T::from_request(req))
            .and_then(move |(params, t)| handler(params, t));
        Box::new(fut)
    }
}

pub struct Server<S>(Arc<InnerServer<S>>);

impl<S> Clone for Server<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

pub struct InnerServer<S> {
    state: Arc<S>,
    methods: HashMap<String, Box<Method<S>>>
}

impl Default for InnerServer<()> {
    fn default() -> Self { InnerServer { state: Arc::new(()), methods: HashMap::new() } }
}

impl Server<()> {
    pub fn new() -> InnerServer<()> { InnerServer::default() }
}

impl<S: 'static + Send + Sync> Server<S> {
    pub fn with_state(state: S) -> InnerServer<S> {
        InnerServer { state: Arc::new(state), methods: HashMap::new() }
    }
}

impl<S: 'static + Send + Sync> InnerServer<S> {
    pub fn with_method<F, I, T, P>(mut self, name: I, handler: F) -> Self
        where
            T: FromRequest<S> + 'static,
            P: DeserializeOwned + 'static + Send + Sync,
            F: Into<With<T, S, P>>,
            I: Into<String> {
            self.methods.insert(name.into(), Box::new(handler.into()));
            self
        }

    pub fn finish(self) -> Server<S> {
        Server(Arc::new(self))
    }
}

impl<S: 'static, WS: 'static> actix_web::dev::Handler<WS> for Server<S> {
    type Result = Box<Future<Item = actix_web::HttpResponse, Error = actix_web::Error>>;

    fn handle(&self, req: &actix_web::HttpRequest<WS>) -> Self::Result {
        use actix_web::FromRequest;
        let cloned = self.clone();
        let rt = bytes::Bytes::extract(req).into_future().and_then(|x| x).and_then(move |bytes| {
            Handler::handle(&cloned, RequestBytes(bytes)).then(|res| match res {
                Ok(res_inner) => {
                    match res_inner {
                        ResponseObjects::Empty => Ok(actix_web::HttpResponse::NoContent().finish()),
                        json => Ok(actix_web::HttpResponse::Ok().json(json))
                    }                    
                }
                Err(_) => Ok(actix_web::HttpResponse::InternalServerError().into())
            })
        });
        Box::new(rt)
    }
}

#[derive(Debug, Deserialize)]
struct ManyRequestObjects<I>(pub I);

#[derive(Debug)]
enum OneOrManyRawValues<'a> {
    Many(Vec<&'a RawValue>),
    One(&'a RawValue)
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

#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseObjects {
    One(Response),
    Many(Vec<Response>),
    Empty
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ManyResponseObjects {
    Many(Vec<Response>),
    Empty
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseObject {
    One(Response),
    Empty
}

impl ResponseObject {
    fn result(result: BoxedSerialize, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| ResponseObject::One(Response::result(result, id)))
            .unwrap_or_else(|| ResponseObject::Empty)
    }

    fn error(error: Error, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| ResponseObject::One(Response::error(error, id)))
            .unwrap_or_else(|| ResponseObject::Empty)
    }
}

pub trait Handler<M> {
    type Result: 'static;
    fn handle(&self, msg: M) -> Self::Result;
}

pub struct RequestBytes(pub bytes::Bytes);

impl<S> Handler<RequestObject> for Server<S>
where S: 'static
{
    type Result = Box<Future<Item = ResponseObject, Error = ()>>;

    fn handle(&self, msg: RequestObject) -> Self::Result {
        let req = WrappedRequestObject { inner: msg, state: Arc::clone(&self.0.state) };

        let opt_id = match req.inner.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None
        };

        let rt = if let Some(method) = self.0.methods.get(req.inner.method.as_ref()) {
            let rt = method.handle(&req).then(|fut| match fut {
                Ok(val) => future_ok(ResponseObject::result(val, opt_id)),
                Err(e) => future_ok(ResponseObject::error(e, opt_id))
            });
            EitherFuture::A(rt)
        } else {
            let rt = future_ok(ResponseObject::error(Error::METHOD_NOT_FOUND, opt_id));
            EitherFuture::B(rt)
        };

        Box::new(rt)
    }
}

impl<S, I> Handler<ManyRequestObjects<I>> for Server<S>
where
    S: 'static,
    I: IntoIterator<Item = RequestObject>
{
    type Result = Box<Future<Item = ManyResponseObjects, Error = ()>>;

    fn handle(&self, msg: ManyRequestObjects<I>) -> Self::Result {
        Box::new(
            futures_unordered(msg.0.into_iter().map(|r| self.handle(r)))
                .filter_map(|res| match res {
                    ResponseObject::One(r) => Some(r),
                    _ => None
                })
                .collect()
                .map(|vec| {
                    if vec.is_empty() {
                        ManyResponseObjects::Empty
                    } else {
                        ManyResponseObjects::Many(vec)
                    }
                })
        )
    }
}

impl<S> Handler<RequestBytes> for Server<S>
where S: 'static
{
    type Result = Box<Future<Item = ResponseObjects, Error = ()>>;

    fn handle(&self, msg: RequestBytes) -> Self::Result {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(msg.0.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return Box::new(future_ok(ResponseObjects::One(Response::error(
                            Error::INVALID_REQUEST,
                            Id::Null
                        ))));
                    }

                    let (okays, errs) = raw_reqs
                        .into_iter()
                        .map(|x| serde_json::from_str::<RequestObject>(x.get()))
                        .partition::<Vec<_>, _>(|x| x.is_ok());

                    let errs = errs
                        .into_iter()
                        .map(|_| Response::error(Error::INVALID_REQUEST, Id::Null))
                        .collect::<Vec<_>>();

                    return Box::new(
                        self.handle(ManyRequestObjects(okays.into_iter().flat_map(|x| x)))
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
                            })
                    );
                }
                OneOrManyRawValues::One(raw_req) => {
                    return Box::new(match serde_json::from_str::<RequestObject>(raw_req.get()) {
                        Ok(rn) => EitherFuture::A(self.handle(rn).map(|res| match res {
                            ResponseObject::One(r) => ResponseObjects::One(r),
                            _ => ResponseObjects::Empty
                        })),
                        Err(_) => EitherFuture::B(future_ok(ResponseObjects::One(Response::error(
                            Error::INVALID_REQUEST,
                            Id::Null
                        ))))
                    });
                }
            }
        }

        Box::new(future_ok(ResponseObjects::One(Response::error(Error::PARSE_ERROR, Id::Null))))
    }
}
