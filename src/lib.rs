use serde::{Serialize, Deserialize, Serializer, Deserializer, de::DeserializeOwned};

use serde_json::{Value, value::RawValue};

use std::sync::Arc;

use futures::{
    future::{
        ok as future_ok, 
        Future, 
        IntoFuture, 
        Either as EitherFuture
    },
    stream::{futures_unordered, Stream},
};

use std::marker::PhantomData;
use std::collections::HashMap;

use erased_serde::Serialize as ErasedSerialize;

use actix::prelude::*;

type BoxedSerialize = Box<ErasedSerialize + Send>;

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
where
    D: Deserializer<'de> {
        Ok(Some(Option::deserialize(deserializer)?))
    }
}

pub struct WrappedRequestObject<S> {
    inner: RequestObject,
    state: Arc<S>
}

#[derive(Deserialize)]
pub struct Params<T>(pub T);

impl<T> Params<T> where T: DeserializeOwned {
    fn from_request_inner(req: &RequestObject) -> Result<Self, Error> {
        let res = match req.params {
            Some(ref raw_value) => serde_json::from_str(raw_value.get()),
            None => serde_json::from_value(Value::Null)
        };

        res
            .map(Params)
            .map_err(|_| Error::INVALID_PARAMS )
    }
}

pub trait FromRequest<S>: Sized {
    type Result: IntoFuture<Item=Self, Error=Error>;
    fn from_request(req: &WrappedRequestObject<S>) -> Self::Result;
}

impl<S> FromRequest<S> for () {
    type Result = Result<Self, Error>;
    fn from_request(_: &WrappedRequestObject<S>) -> Self::Result {
        Ok(())
    }
}

pub struct State<S>(Arc<S>);

impl<S> std::ops::Deref for State<S> {
    type Target = S;
    fn deref(&self) -> &S {
        &*self.0
    }
}

impl<S> FromRequest<S> for State<S> {
    type Result = Result<Self, Error>;
    fn from_request(req: &WrappedRequestObject<S>) -> Self::Result {
        Ok(State(Arc::clone(&req.state)))
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
    PreDef { code: i64, message: &'static str },
}

impl Error {
    pub const INVALID_REQUEST: Self = Error::PreDef { code: -32600, message: "Invalid Request" };
    pub const METHOD_NOT_FOUND: Self = Error::PreDef { code: -32601, message: "Method not found" };
    pub const INVALID_PARAMS: Self = Error::PreDef { code: -32602, message: "Invalid params" };
    pub const PARSE_ERROR: Self = Error::PreDef { code: -32700, message: "Parse error" };
}

pub trait ErrorLike {
    fn code(&self) -> i64;
    fn message(&self) -> String;
    fn data(&self) -> Option<BoxedSerialize> {
        None
    }
}

impl<T> From<T> for Error where T: ErrorLike {
    fn from(t: T) -> Error {
        Error::Full {
            code: t.code(),
            message: t.message(),
            data: t.data()
        }
    }
}

#[cfg(feature="easy_errors")]
impl<T> ErrorLike for T where T: std::fmt::Display {
    fn code(&self) -> i64 { 0 }
    fn message(&self) -> String { self.to_string() }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Response {
    Result { jsonrpc: V2, result: BoxedSerialize, id: Id },
    Error { jsonrpc: V2, error: Error, id: Id },
    Empty
}

impl Response {
    fn result(result: BoxedSerialize, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| Response::Result { jsonrpc: V2, result, id })
            .unwrap_or_else(|| Response::Empty)
    }

    fn error(error: Error, opt_id: Option<Id>) -> Self {
        opt_id
            .map(|id| Response::Error { jsonrpc: V2, error, id })
            .unwrap_or_else(|| Response::Empty)
    }
}

pub trait WithFactory<T, S, P>: 'static
where
    T: FromRequest<S>,
    P: DeserializeOwned
{
    fn create(self) -> With<T, S, P>;
}

pub struct With<T, S, P> {
    handler: Arc<Fn(Params<P>, T) -> Box<Future<Item=BoxedSerialize, Error=Error>> + Send + Sync>,
    _s: PhantomData<S>,
    _p: PhantomData<P>
}

impl<T, S, P> With<T, S, P>
where
    T: FromRequest<S>,
    P: DeserializeOwned,
    S: 'static,
{
    pub fn new<F: Fn(Params<P>, T) -> Box<Future<Item=BoxedSerialize, Error=Error>> + 'static + Send + Sync>(f: F) -> Self {
        With {
            handler: Arc::new(f),
            _s: PhantomData,
            _p: PhantomData
        }
    }
}

impl<S, P, FN, I, E, FS, T> WithFactory<T, S, P> for FN
where
    T: FromRequest<S> + 'static,
    S: 'static,
    P: DeserializeOwned,
    FN: Fn(Params<P>, T) -> I + 'static + Send + Sync,
    I: IntoFuture<Item = FS, Error = E> + 'static,
    I::Future: 'static,
    FS: Serialize + Send + 'static,
    E: Into<Error> {

    fn create(self) -> With<T, S, P> {
        With::new(move |params, t| {
            let rt = (self)(params, t).into_future()
                .map_err(|e| e.into() )
                .map(|res| Box::new(res) as BoxedSerialize);

            Box::new(rt) as Box<Future<Item=BoxedSerialize, Error=Error>>
        })
    }
}

pub trait Method<S>: 'static + Send + Sync {
    fn handle(&self, req: &WrappedRequestObject<S>) -> Box<Future<Item=BoxedSerialize, Error=Error>>;
}

impl<T, S, P> Method<S> for With<T, S, P>
where
    T: FromRequest<S> + 'static,
    P: DeserializeOwned,
    P: 'static + Send + Sync,
    S: 'static + Send + Sync,
{
    fn handle(&self, req: &WrappedRequestObject<S>) -> Box<Future<Item=BoxedSerialize, Error=Error>> {        
        let handler = Arc::clone(&self.handler);
        let fut = Params::from_request_inner(&req.inner).into_future()
            .join(T::from_request(req))
            .and_then(move |(params, t)| handler(params, t) );
        Box::new(fut)
    }
}

pub struct Server<S> {
    state: Arc<S>,
    methods: HashMap<String, Box<Method<S>>>
}

impl Server<()> {
    pub fn new() -> Self { Server { state: Arc::new(()), methods: HashMap::new() } }
}

impl<S: 'static + Send + Sync> Server<S> {
    pub fn with_state(state: S) -> Self {
        Server { state: Arc::new(state), methods: HashMap::new() }
    }

    pub fn with_method<T, P, F>(mut self, name: String, handler: F) -> Self
    where
        F: WithFactory<T, S, P>,
        T: FromRequest<S> + 'static,
        P: DeserializeOwned + 'static + Send + Sync
    {
        self.methods.insert(name, Box::new(handler.create()));
        self
    }

}

#[derive(Debug, Deserialize)]
pub struct ManyRequestObjects(pub Vec<RequestObject>);

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

impl From<Response> for ResponseObjects {
    fn from(t: Response) -> Self {
        match t {
            Response::Empty => ResponseObjects::Empty,
            t => ResponseObjects::One(t)
        }
    }
}

impl From<Vec<Response>> for ResponseObjects {
    fn from(t: Vec<Response>) -> Self {
        let t = t
            .into_iter()
            .filter_map(|r| match r {
                Response::Empty => None,
                t => Some(t)
            })
            .collect::<Vec<_>>();
        if t.is_empty() {
            return ResponseObjects::Empty;
        }
        ResponseObjects::Many(t)
    }
}

pub struct RequestBytes(pub bytes::Bytes);

impl<S> Actor for Server<S>
where
    S: 'static {
    type Context = Context<Self>;
}

impl Message for RequestObject {
    type Result = Result<Response, ()>;
}

impl Message for ManyRequestObjects {
    type Result = Result<ResponseObjects, ()>;
}

impl Message for RequestBytes {
    type Result = Result<ResponseObjects, ()>;
}

impl<S> Handler<RequestObject> for Server<S>
where
    S: 'static {
    type Result = Box<Future<Item=Response, Error=()>>;

    fn handle(&mut self, msg: RequestObject, _: &mut Self::Context) -> Self::Result {
        let req = WrappedRequestObject { inner: msg, state: Arc::clone(&self.state) };

        let opt_id = match req.inner.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None
        };

        let rt = if let Some(method) = self.methods.get(req.inner.method.as_ref()) {
            let rt = method.handle(&req).then(|fut| match fut {
                Ok(val) => future_ok(Response::result(val, opt_id)),
                Err(e) => future_ok(Response::error(e, opt_id))
            });
            EitherFuture::A(rt)
        } else {
            let rt = future_ok(Response::error(Error::METHOD_NOT_FOUND, opt_id));
            EitherFuture::B(rt)
        };

        Box::new(rt)
    }
}

impl<S> Handler<ManyRequestObjects> for Server<S>
where
    S: 'static {
    type Result = Box<Future<Item=ResponseObjects, Error=()>>;

    fn handle(&mut self, msg: ManyRequestObjects, ctx: &mut Self::Context) -> Self::Result {        
        Box::new(futures_unordered(msg.0.into_iter().map(|r| self.handle(r, ctx)))
            .collect()
            .map(ResponseObjects::from))
    }
}

impl<S> Handler<RequestBytes> for Server<S>
where
    S: 'static {
    type Result = Box<Future<Item=ResponseObjects, Error=()>>;

    fn handle(&mut self, msg: RequestBytes, ctx: &mut Self::Context) -> Self::Result {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(msg.0.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return Box::new(future_ok(
                            Response::error(Error::INVALID_REQUEST, Some(Id::Null)).into()
                        ));
                    }

                    let (okays, errs) = raw_reqs
                        .into_iter()
                        .map(|x| serde_json::from_str::<RequestObject>(x.get()))
                        .partition::<Vec<Result<RequestObject, serde_json::Error>>, _>(|x| {
                            x.is_ok()
                        });

                    let errs = errs
                        .into_iter()
                        .map(|_| Response::error(Error::INVALID_REQUEST, Some(Id::Null)))
                        .collect::<Vec<_>>();

                    return Box::new(
                        self.handle(ManyRequestObjects(okays.into_iter().flat_map(|x| x).collect()), ctx).map(
                            |res| {
                                match res {
                                    ResponseObjects::One(one) => {
                                        let mut many = vec![one];
                                        many.extend(errs);
                                        many.into()
                                    },
                                    ResponseObjects::Many(mut many) => {
                                        many.extend(errs);
                                        many.into()
                                    },
                                    ResponseObjects::Empty => { errs.into() }
                                }
                            }
                        )
                    );
                },
                OneOrManyRawValues::One(raw_req) => {
                    return Box::new(match serde_json::from_str::<RequestObject>(raw_req.get()) {
                        Ok(rn) => EitherFuture::A(self.handle(rn, ctx)),
                        Err(_) => EitherFuture::B(future_ok(
                            Response::error(Error::INVALID_REQUEST, Some(Id::Null)).into()
                        ))
                    }.map(ResponseObjects::from));
                }
            }
            
        }

        Box::new(future_ok(Response::error(Error::PARSE_ERROR, Some(Id::Null)).into()))
    }
}



