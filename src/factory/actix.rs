use futures::Future;

#[doc(hidden)]
#[async_trait::async_trait(?Send)]
pub trait Factory<S, E, T> {
    async fn call(&self, param: T) -> Result<S, E>;
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E> Factory<S, E, ()> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
    FN: Fn() -> I + Sync,
{
    async fn call(&self, _: ()) -> Result<S, E> {
        (self)().await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1> Factory<S, E, (T1,)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
    FN: Fn(T1) -> I + Sync,
    T1: Send + 'static,
{
    async fn call(&self, param: (T1,)) -> Result<S, E> {
        (self)(param.0).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2> Factory<S, E, (T1, T2)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
    FN: Fn(T1, T2) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
{
    async fn call(&self, param: (T1, T2)) -> Result<S, E> {
        (self)(param.0, param.1).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3> Factory<S, E, (T1, T2, T3)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
    FN: Fn(T1, T2, T3) -> I + Sync,
    T1: Send + 'static,
    T2: Send + 'static,
    T3: Send + 'static,
{
    async fn call(&self, param: (T1, T2, T3)) -> Result<S, E> {
        (self)(param.0, param.1, param.2).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3, T4> Factory<S, E, (T1, T2, T3, T4)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
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

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3, T4, T5> Factory<S, E, (T1, T2, T3, T4, T5)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>> + 'static,
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
