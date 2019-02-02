use std::borrow::{Borrow, Cow};

use serde::{Serialize, Deserialize, Serializer, Deserializer, de::DeserializeOwned};

use serde_json::{Value, value::RawValue};

use std::rc::Rc;
use std::sync::Arc;

use futures::future::{err as future_err, ok as future_ok, Future, IntoFuture, Either as EitherFuture, FutureResult};
use futures::{Async, Poll};

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

#[derive(Debug, Clone)]
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

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        match *self {
            Id::Num(ref num) => num.serialize(serializer),
            Id::Str(ref s) => s.serialize(serializer),
            Id::Null => serializer.serialize_none()
        }
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Id, D::Error>
    where D: Deserializer<'de> {
        
        #[derive(Serialize, Deserialize)]
        #[serde(untagged)]
        pub enum PresentId {
            Num(i64),
            Str(Box<str>)
        }

        let out = match <Option<PresentId>>::deserialize(deserializer)? {
            Some(PresentId::Num(num)) => Id::Num(num),
            Some(PresentId::Str(s)) => Id::Str(s),
            None => Id::Null
        };

        Ok(out)
    }
}

#[derive(Deserialize)]
pub struct Params<T>(pub T);

impl<T> Params<T> where T: DeserializeOwned {
    fn from_opt_value(value_opt: Option<Value>) -> Result<Self, Error> {
        serde_json::from_value(value_opt.unwrap_or_else(|| Value::Null))
            .map(Params)
            .map_err(|_| Error::INVALID_PARAMS )
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RawRequestObject {
    Request { 
        jsonrpc: V2, 
        method: Box<str>, 
        params: Option<Value>,
        id: Id
    },
    Notification {
        jsonrpc: V2, 
        method: Box<str>,
        params: Option<Value>,
    }
}

pub struct RequestObject<S> {
    inner: RawRequestObject,
    state: Rc<S>
}

pub trait FromRequest<S>: Sized {
    type Result: IntoFuture<Item=Self, Error=Error>;
    fn from_request(req: &RequestObject<S>) -> Self::Result;
}

impl<S> FromRequest<S> for () {
    type Result = Result<Self, Error>;
    fn from_request(req: &RequestObject<S>) -> Self::Result {
        Ok(())
    }
}

pub struct State<S>(Rc<S>);

impl<S> std::ops::Deref for State<S> {
    type Target = S;
    fn deref(&self) -> &S {
        &*self.0
    }
}

impl<S> FromRequest<S> for State<S> {
    type Result = Result<Self, Error>;
    fn from_request(req: &RequestObject<S>) -> Self::Result {
        Ok(State(Rc::clone(&req.state)))
    }
}


#[derive(Serialize)]
#[serde(untagged)]
pub enum Error {
    Full { code: i64, message: String, data: Option<BoxedSerialize> },
    PreDef { code: i64, message: &'static str },
}

impl Error {
    pub const METHOD_NOT_FOUND: Self = Error::PreDef { code: -32601, message: "Method not found" };
    pub const INVALID_PARAMS: Self = Error::PreDef { code: -32602, message: "Invalid params" };
    pub const INTERNAL_ERROR: Self = Error::PreDef { code: 1, message: "Could not process request" };
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
    handler: Rc<Fn(Params<P>, T) -> Box<Future<Item=BoxedSerialize, Error=Error>>>,
    _s: PhantomData<S>,
    _p: PhantomData<P>
}

impl<T, S, P> With<T, S, P>
where
    T: FromRequest<S>,
    P: DeserializeOwned,
    S: 'static,
{
    pub fn new<F: Fn(Params<P>, T) -> Box<Future<Item=BoxedSerialize, Error=Error>> + 'static>(f: F) -> Self {
        With {
            handler: Rc::new(f),
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
    FN: Fn(Params<P>, T) -> I + 'static,
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

pub trait MethodHandler<S>: 'static {
    fn handle(&self, req: &RequestObject<S>, opt_value: Option<Value>) -> Box<Future<Item=BoxedSerialize, Error=Error>>;
}

impl<T, S, P> MethodHandler<S> for With<T, S, P>
where
    T: FromRequest<S> + 'static,
    P: DeserializeOwned,
    P: 'static,
    S: 'static,
{
    fn handle(&self, req: &RequestObject<S>, opt_value: Option<Value>) -> Box<Future<Item=BoxedSerialize, Error=Error>> {        
        let handler = Rc::clone(&self.handler);
        let fut = Params::from_opt_value(opt_value).into_future()
            .join(T::from_request(&req))
            .and_then(move |(params, t)| handler(params, t) );
        Box::new(fut)
    }
}

pub struct Server<S> {
    state: Rc<S>,
    methods: HashMap<String, Box<MethodHandler<S>>>
}

impl Server<()> {
    pub fn new() -> Self { Server { state: Rc::new(()), methods: HashMap::new() } }
}

impl<S: 'static> Server<S> {
    pub fn with_state<NS>(mut self, state: NS) -> Server<NS> {
        let Server { .. } = self;
        let state = Rc::new(state);
        Server { state, methods: HashMap::new() }
    }

    pub fn with_method<T, P, F>(mut self, name: String, handler: F) -> Self
    where
        F: WithFactory<T, S, P>,
        T: FromRequest<S> + 'static,
        P: DeserializeOwned,
        P: 'static
    {
        self.methods.insert(name, Box::new(handler.create()));
        self
    }

}

impl<S> Actor for Server<S>
where
    S: 'static {
    type Context = Context<Self>;
}

impl Message for RawRequestObject {
    type Result = Result<Response, ()>;
}

impl<S> Handler<RawRequestObject> for Server<S>
where
    S: 'static {
    type Result = Box<Future<Item=Response, Error=()>>;

    fn handle(&mut self, req: RawRequestObject, _: &mut Self::Context) -> Self::Result {
        let mut req = RequestObject { inner: req, state: Rc::clone(&self.state) };

        let (opt_id, method_ref) = match req.inner {
            RawRequestObject::Request { ref method, ref id, .. } => (Some(id.clone()), method),
            RawRequestObject::Notification { ref method, .. } => (None, method),
        };

        let rt = if let Some(method) = self.methods.get(method_ref.as_ref()) {
            
            let params_opt = match req.inner {
                RawRequestObject::Request { ref mut params, .. } => params.take(),
                RawRequestObject::Notification { ref mut params, .. } => params.take()
            };

            let rt = method.handle(&req, params_opt).then(|fut| match fut {
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


// pub struct BoxedMethod<S> {
//     inner: Box<Fn(RequestObject<S>) -> Box<Future<Item=BoxedSerialize, Error=Error>>>,
// }

// pub trait Args: Sized { }
// impl Args for () { }


// pub trait IntoBoxedMethod<S> {
//     type Args: Args;
//     fn into_boxed_method(&self) -> BoxedMethod<S>;
// }

// impl<'de, ST, U, F, S, I, E> IntoBoxedMethod<ST> for F where
//     F: Fn(Params<U>) -> I + 'static,
//     Params<U>: FromRequest<ST>,
//     U: 'static,
//     I: IntoFuture<Item = S, Error = E> + 'static + Send,
//     I::Future: 'static + Send,
//     S: Serialize + 'static,
//     E: Into<Error> {

//     fn into_boxed_method(&self) -> BoxedMethod<ST> {
//         let rt = move |req| {
//             let rt = Params::from_request(&req).into_future()
//                 .and_then(move |params| {
//                     self(params).into_future()
//                         .map_err(|e| e.into())
//                         .map(|r| {
//                             Box::new(r) as BoxedSerialize
//                         })
//                 });
//             Box::new(rt) as Box<Future<Item=BoxedSerialize, Error=Error>>
//         };

//         BoxedMethod { inner: Box::new(rt) }
//     }
// }

// pub struct AsyncResult<I, E=Error>(Option<AsyncResultItem<I, E>>);

// impl<I, E> Future for AsyncResult<I, E> {
//     type Item = I;
//     type Error = E;

//     fn poll(&mut self) -> Poll<I, E> {
//         let res = self.0.take().expect("use after resolve");
//         match res {
//             AsyncResultItem::Ok(msg) => Ok(Async::Ready(msg)),
//             AsyncResultItem::Err(err) => Err(err),
//             AsyncResultItem::Future(mut fut) => match fut.poll() {
//                 Ok(Async::NotReady) => {
//                     self.0 = Some(AsyncResultItem::Future(fut));
//                     Ok(Async::NotReady)
//                 }
//                 Ok(Async::Ready(msg)) => Ok(Async::Ready(msg)),
//                 Err(err) => Err(err),
//             },
//         }
//     }
// }

// pub(crate) enum AsyncResultItem<I, E> {
//     Ok(I),
//     Err(E),
//     Future(Box<Future<Item = I, Error = E>>),
// }

// impl<T> From<T> for AsyncResult<T> {
//     #[inline]
//     fn from(resp: T) -> AsyncResult<T> {
//         AsyncResult(Some(AsyncResultItem::Ok(resp)))
//     }
// }

// impl<T, E: Into<Error>> From<Result<AsyncResult<T>, E>> for AsyncResult<T> {
//     #[inline]
//     fn from(res: Result<AsyncResult<T>, E>) -> Self {
//         match res {
//             Ok(val) => val,
//             Err(err) => AsyncResult(Some(AsyncResultItem::Err(err.into()))),
//         }
//     }
// }

// impl<T, E: Into<Error>> From<Result<T, E>> for AsyncResult<T> {
//     #[inline]
//     fn from(res: Result<T, E>) -> Self {
//         match res {
//             Ok(val) => AsyncResult(Some(AsyncResultItem::Ok(val))),
//             Err(err) => AsyncResult(Some(AsyncResultItem::Err(err.into()))),
//         }
//     }
// }

// impl<T, E> From<Result<Box<Future<Item = T, Error = E>>, E>> for AsyncResult<T>
// where
//     T: 'static,
//     E: Into<Error> + 'static,
// {
//     #[inline]
//     fn from(res: Result<Box<Future<Item = T, Error = E>>, E>) -> Self {
//         match res {
//             Ok(fut) => AsyncResult(Some(AsyncResultItem::Future(Box::new(
//                 fut.map_err(|e| e.into()),
//             )))),
//             Err(err) => AsyncResult(Some(AsyncResultItem::Err(err.into()))),
//         }
//     }
// }

// impl<T> From<Box<Future<Item = T, Error = Error>>> for AsyncResult<T> {
//     #[inline]
//     fn from(fut: Box<Future<Item = T, Error = Error>>) -> AsyncResult<T> {
//         AsyncResult(Some(AsyncResultItem::Future(fut)))
//     }
// }

// pub trait FromRequest<'a, S>: Sized {
//     type Result: Into<AsyncResult<Self>>;
//     fn from_request(request: &RequestObject<'a, S>) -> Self::Result;
// }

// impl<'a, S, T> FromRequest<'a, S> for Params<T> where T: Deserialize<'a> {
//     type Result = Result<Self, Error>;
//     fn from_request(request: &RequestObject<'a, S>) -> Self::Result {
//         request.inner.extract_params()
//             .map(Params)
//             .map_err(|_| Error::INVALID_PARAMS )
//     }
// }

// pub struct State<S>(Rc<S>);

// impl<S> std::ops::Deref for State<S> {
//     type Target = S;
//     fn deref(&self) -> &S {
//         &*self.0
//     }
// }

// impl<'a, S> FromRequest<'a, S> for State<S> {
//     type Result = Result<Self, Error>;
//     fn from_request(request: &RequestObject<'a, S>) -> Self::Result {
//         Ok(State(request.state.clone()))
//     }
// }

// pub struct Server<S> {
//     state: Rc<S>
// }

// impl Server<()> {
//     pub fn new() -> Self { Server { state: Rc::new(()) } }
// }

// pub struct BoxedMethod<'a, S> {
//     inner: Box<FnOnce(RequestObject<'a, S>) -> Box<Future<Item=Box<erased_serde::Serialize>, Error=Error>>>
// }

// pub trait IntoBoxedMethod<'a, S, T> {
//     fn into_boxed_method(self) -> BoxedMethod<'a, S>;
// }

// impl<'a, ST, U, F, S, I, E> IntoBoxedMethod<'a, ST, Params<U>> for F where
//     F: Fn(Params<U>) -> I + 'static,
//     Params<U>: FromRequest<'a, ST>,
//     U: 'static,
//     I: IntoFuture<Item = S, Error = E> + 'static + Send,
//     I::Future: 'static + Send,
//     S: Serialize + 'static,
//     E: Into<Error> {

//     fn into_boxed_method(self) -> BoxedMethod<'a, ST> {
//         let rt = move |req| {
//             let rt = Params::from_request(&req).into()
//                 .and_then(move |params| {
//                     self(params).into_future()
//                         .map_err(|e| e.into())
//                         .map(|r| {
//                             Box::new(r) as Box<erased_serde::Serialize>
//                         })
//                 });
//             Box::new(rt) as Box<Future<Item=Box<erased_serde::Serialize>, Error=Error>>
//         };

//         BoxedMethod { inner: Box::new(rt) }
//     }
// }

// trait FnWith<T, R>: 'static {
//     fn call_with(self: &Self, arg: T) -> R;
// }

// impl<T, R, F: Fn(T) -> R + 'static> FnWith<T, R> for F {
//     fn call_with(self: &Self, arg: T) -> R {
//         (*self)(arg)
//     }
// }

// pub trait WithFactory<'a, T, S>: 'static
// where
//     T: FromRequest<'a, S>
// {
//     fn create(self) -> With<T, S>;
// }

// pub struct With<T, S> {
//     hnd: Box<FnWith<T, AsyncResult<Box<erased_serde::Serialize>>>>,
//     _s: PhantomData<S>,
// }

// impl<'a, T, S> With<T, S>
// where
//     T: FromRequest<'a, S>,
//     S: 'static,
// {
//     pub fn new<F: Fn(T) -> AsyncResult<Box<erased_serde::Serialize>> + 'static>(f: F) -> Self {
//         With {
//             hnd: Box::new(f),
//             _s: PhantomData,
//         }
//     }
// }

// impl<'a, T, S, F, IF> WithFactory<'a, T, S> for F where 
//     F: Fn(Params<U>) -> IF + 'static,
//     SS: Serialize,
//     IF: Into<AsyncResult<SS>>,
//     {
//         fn create(self) -> With<T, S> {
//             With::new(move |a| (self)(a))
//         }
//     }


// pub trait Method<'a, S>: Sized {
//     fn respond_to(self, request: RequestObject<'a, S>) -> AsyncResult<Box<erased_serde::Serialize>>;
// }



// impl<S> Server<S> {
//     pub fn with_state<NS>(mut self, state: NS) -> Server<NS> {
//         let Server { .. } = self;
//         let state = Rc::new(state);
//         Server { state }
//     }

//     // pub fn with_method(mut self, state: NS) -> Server<NS> {

//     // }
// }


// pub struct Empty {
//     foo: String,
//     bar: i32
// }

// pub struct Empty2 {
//     foo: String,
//     bar: i32
// }