//! Types and utilities for dependency injection

use super::error::{Error, ErrorCode};
pub use volga_di::{
    ContainerBuilder, 
    Container, 
    GenericFactory, 
    Inject
};

pub use volga_di::error::Error as DiError;
pub use dc::Dc;

mod dc;

impl From<DiError> for Error {
    #[inline]
    fn from(err: DiError) -> Self {
        Self::new(ErrorCode::InternalError, err.to_string())
    }
}

#[cfg(feature = "server")]
impl super::App {
    /// Registers singleton service
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// #[derive(Default, Clone)]
    /// struct Singleton;
    ///
    /// let singleton = Singleton::default();
    /// App::new()
    ///     .add_singleton(singleton);
    /// ```
    pub fn add_singleton<T: Send + Sync + 'static>(mut self, instance: T) -> Self {
        self.container.register_singleton(instance);
        self
    }

    /// Registers scoped service
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, di::{Container, Inject, DiError}};
    ///
    /// #[derive(Clone)]
    /// struct ScopedService;
    ///
    /// impl Inject for ScopedService {
    ///     fn inject(_: &Container) -> Result<Self, DiError> {
    ///         Ok(Self)
    ///     }
    /// }
    ///
    /// App::new()
    ///     .add_scoped::<ScopedService>();
    /// ```
    pub fn add_scoped<T: Inject + 'static>(mut self) -> Self {
        self.container.register_scoped::<T>();
        self
    }

    /// Registers scoped service that required to be resolved via factory
    ///
    /// > **Note:** Provided factory function will be called once per scope 
    /// > and the result will be available and reused per this scope lifetime.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// #[derive(Clone)]
    /// struct ScopedService;
    ///
    /// impl ScopedService {
    ///     fn new() -> Self {
    ///         ScopedService
    ///     }
    /// }
    ///
    /// App::new()
    ///     .add_scoped_factory(|| ScopedService::new());
    /// ```
    pub fn add_scoped_factory<T, F, Args>(mut self, factory: F) -> Self
    where
        T: Send + Sync + 'static,
        F: GenericFactory<Args, Output = T>,
        Args: Inject
    {
        self.container.register_scoped_factory(factory);
        self
    }

    /// Registers scoped service that required to be resolved as [`Default`]
    ///
    /// > **Note:** the [`Default::default`] method will be called once per scope 
    /// > and the result will be available and reused per this scope lifetime.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// #[derive(Default, Clone)]
    /// struct ScopedService;
    ///
    /// App::new()
    ///     .add_scoped_default::<ScopedService>();
    /// ```
    pub fn add_scoped_default<T>(mut self) -> Self
    where
        T: Default + Send + Sync + 'static,
    {
        self.container.register_scoped_default::<T>();
        self
    }

    /// Registers transient service
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, di::{Container, Inject, DiError}};
    ///
    /// #[derive(Clone)]
    /// struct TransientService;
    ///
    /// impl Inject for TransientService {
    ///     fn inject(_: &Container) -> Result<Self, DiError> {
    ///         Ok(Self)
    ///     }
    /// }
    ///
    /// App::new()
    ///     .add_transient::<TransientService>();
    /// ```
    pub fn add_transient<T: Inject + 'static>(mut self) -> Self {
        self.container.register_transient::<T>();
        self
    }

    /// Registers transient service that required to be resolved via factory
    ///
    /// > **Note:** Provided factory function will be called 
    /// > every time once this service is requested.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// #[derive(Clone)]
    /// struct TransientService;
    ///
    /// impl TransientService {
    ///     fn new() -> Self {
    ///         TransientService
    ///     }
    /// }
    ///
    /// App::new()
    ///     .add_transient_factory(|| TransientService::new());
    /// ```
    pub fn add_transient_factory<T, F, Args>(mut self, factory: F) -> Self
    where
        T: Send + Sync + 'static,
        F: GenericFactory<Args, Output = T>,
        Args: Inject
    {
        self.container.register_transient_factory(factory);
        self
    }

    /// Registers transient service that required to be resolved as [`Default`]
    ///
    /// > **Note:** the [`Default::default`] method will be called 
    /// > every time once this service is requested.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// #[derive(Default, Clone)]
    /// struct TransientService;
    ///
    /// App::new()
    ///     .add_transient_default::<TransientService>();
    /// ```
    pub fn add_transient_default<T>(mut self) -> Self
    where
        T: Default + Send + Sync + 'static,
    {
        self.container.register_transient_default::<T>();
        self
    }
}

#[cfg(feature = "client")]
impl super::Client {

}

#[cfg(feature = "server")]
#[cfg(test)]
mod tests {
    use volga_di::{Container, Inject};
    use super::super::App;

    #[derive(Default)]
    struct TestDependency;

    impl Inject for TestDependency {
        fn inject(_: &Container) -> Result<Self, volga_di::error::Error> {
            Ok(TestDependency)
        }
    }

    #[test]
    fn it_adds_singleton() {
        let mut app = App::new();
        app = app.add_singleton(TestDependency);

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_scoped() {
        let mut app = App::new();
        app = app.add_scoped::<TestDependency>();

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_scoped_factory() {
        let mut app = App::new();
        app = app.add_scoped_factory(|| TestDependency);

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_scoped_default() {
        let mut app = App::new();
        app = app.add_scoped_default::<TestDependency>();

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_transient() {
        let mut app = App::new();
        app = app.add_transient::<TestDependency>();

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_transient_factory() {
        let mut app = App::new();
        app = app.add_transient_factory(|| TestDependency);

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }

    #[test]
    fn it_adds_transient_default() {
        let mut app = App::new();
        app = app.add_transient_default::<TestDependency>();

        let container = app.container.build();
        let dep = container.resolve_shared::<TestDependency>();

        assert!(dep.is_ok());
    }
}