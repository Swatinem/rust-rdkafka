//! Admin client.
//!
//! The main object is the [`AdminClient`] struct.
//!
//! [`AdminClient`]: struct.AdminClient.html

use crate::rdsys;
use crate::rdsys::types::*;

use crate::client::{Client, ClientContext, DefaultClientContext, NativeQueue};
use crate::config::{ClientConfig, FromClientConfig, FromClientConfigAndContext};
use crate::error::{IsError, KafkaError, KafkaResult};
use crate::util::{cstr_to_owned, timeout_to_ms, AsCArray, ErrBuf, IntoOpaque, WrappedCPointer};

use futures::future::{self, Either};
use futures::{Async, Canceled, Complete, Future, Oneshot, Poll};

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

//
// ********** ADMIN CLIENT **********
//

/// A client for the Kafka admin API.
///
/// `AdminClient` provides programmatic access to managing a Kafka cluster,
/// notably manipulating topics, partitions, and configuration paramaters.
pub struct AdminClient<C: ClientContext> {
    client: Client<C>,
    queue: Arc<NativeQueue>,
    should_stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl<C: ClientContext> AdminClient<C> {
    /// Creates new topics according to the provided `NewTopic` specifications.
    ///
    /// Note that while the API supports creating multiple topics at once, it
    /// is not transactional. Creation of some topics may succeed while others
    /// fail. Be sure to check the result of each individual operation.
    pub fn create_topics<'a, I>(
        &self,
        topics: I,
        opts: &AdminOptions,
    ) -> impl Future<Item = Vec<TopicResult>, Error = KafkaError>
    where
        I: IntoIterator<Item = &'a NewTopic<'a>>,
    {
        match self.create_topics_inner(topics, opts) {
            Ok(rx) => Either::A(CreateTopicsFuture { rx }),
            Err(err) => Either::B(future::err(err)),
        }
    }

    fn create_topics_inner<'a, I>(
        &self,
        topics: I,
        opts: &AdminOptions,
    ) -> KafkaResult<Oneshot<NativeEvent>>
    where
        I: IntoIterator<Item = &'a NewTopic<'a>>,
    {
        let mut native_topics = Vec::new();
        let mut err_buf = ErrBuf::new();
        for t in topics {
            native_topics.push(t.to_native(&mut err_buf)?);
        }
        let (native_opts, rx) = opts.to_native(self.client.native_ptr(), &mut err_buf)?;
        unsafe {
            rdsys::rd_kafka_CreateTopics(
                self.client.native_ptr(),
                native_topics.as_c_array(),
                native_topics.len(),
                native_opts.ptr(),
                self.queue.ptr(),
            );
        }
        Ok(rx)
    }

    /// Deletes the named topics.
    ///
    /// Note that while the API supports deleting multiple topics at once, it is
    /// not transactional. Deletion of some topics may succeed while others
    /// fail. Be sure to check the result of each individual operation.
    pub fn delete_topics(
        &self,
        topic_names: &[&str],
        opts: &AdminOptions,
    ) -> impl Future<Item = Vec<TopicResult>, Error = KafkaError> {
        match self.delete_topics_inner(topic_names, opts) {
            Ok(rx) => Either::A(DeleteTopicsFuture { rx }),
            Err(err) => Either::B(future::err(err)),
        }
    }

    fn delete_topics_inner(
        &self,
        topic_names: &[&str],
        opts: &AdminOptions,
    ) -> KafkaResult<Oneshot<NativeEvent>> {
        let mut native_topics = Vec::new();
        let mut err_buf = ErrBuf::new();
        for tn in topic_names {
            let tn_c = CString::new(*tn)?;
            let native_topic = unsafe {
                NativeDeleteTopic::from_ptr(rdsys::rd_kafka_DeleteTopic_new(tn_c.as_ptr()))
            };
            native_topics.push(native_topic);
        }
        let (native_opts, rx) = opts.to_native(self.client.native_ptr(), &mut err_buf)?;
        unsafe {
            rdsys::rd_kafka_DeleteTopics(
                self.client.native_ptr(),
                native_topics.as_c_array(),
                native_topics.len(),
                native_opts.ptr(),
                self.queue.ptr(),
            );
        }
        Ok(rx)
    }

    /// Adds additional partitions to existing topics according to the provided
    /// `NewPartitions` specifications.
    ///
    /// Note that while the API supports creating partitions for multiple topics
    /// at once, it is not transactional. Creation of partitions for some topics
    /// may succeed while others fail. Be sure to check the result of each
    /// individual operation.
    pub fn create_partitions<'a, I>(
        &self,
        partitions: I,
        opts: &AdminOptions,
    ) -> impl Future<Item = Vec<TopicResult>, Error = KafkaError>
    where
        I: IntoIterator<Item = &'a NewPartitions<'a>>,
    {
        match self.create_partitions_inner(partitions, opts) {
            Ok(rx) => Either::A(CreatePartitionsFuture { rx }),
            Err(err) => Either::B(future::err(err)),
        }
    }

    fn create_partitions_inner<'a, I>(
        &self,
        partitions: I,
        opts: &AdminOptions,
    ) -> KafkaResult<Oneshot<NativeEvent>>
    where
        I: IntoIterator<Item = &'a NewPartitions<'a>>,
    {
        let mut native_partitions = Vec::new();
        let mut err_buf = ErrBuf::new();
        for p in partitions {
            native_partitions.push(p.to_native(&mut err_buf)?);
        }
        let (native_opts, rx) = opts.to_native(self.client.native_ptr(), &mut err_buf)?;
        unsafe {
            rdsys::rd_kafka_CreatePartitions(
                self.client.native_ptr(),
                native_partitions.as_c_array(),
                native_partitions.len(),
                native_opts.ptr(),
                self.queue.ptr(),
            );
        }
        Ok(rx)
    }

    /// Retrieves the configuration parameters for the specified resources.
    ///
    /// Note that while the API supports describing multiple configurations at
    /// once, it is not transactional. There is no guarantee that you will see
    /// a consistent snapshot of the configuration across different resources.
    pub fn describe_configs<'a, I>(
        &self,
        configs: I,
        opts: &AdminOptions,
    ) -> impl Future<Item = Vec<ConfigResourceResult>, Error = KafkaError>
    where
        I: IntoIterator<Item = &'a ResourceSpecifier<'a>>,
    {
        match self.describe_configs_inner(configs, opts) {
            Ok(rx) => Either::A(DescribeConfigsFuture { rx }),
            Err(err) => Either::B(future::err(err)),
        }
    }

    fn describe_configs_inner<'a, I>(
        &self,
        configs: I,
        opts: &AdminOptions,
    ) -> KafkaResult<Oneshot<NativeEvent>>
    where
        I: IntoIterator<Item = &'a ResourceSpecifier<'a>>,
    {
        let mut native_configs = Vec::new();
        let mut err_buf = ErrBuf::new();
        for c in configs {
            let (name, typ) = match c {
                ResourceSpecifier::Topic(name) => (
                    CString::new(*name)?,
                    RDKafkaResourceType::RD_KAFKA_RESOURCE_TOPIC,
                ),
                ResourceSpecifier::Group(name) => (
                    CString::new(*name)?,
                    RDKafkaResourceType::RD_KAFKA_RESOURCE_GROUP,
                ),
                ResourceSpecifier::Broker(id) => (
                    CString::new(format!("{}", id))?,
                    RDKafkaResourceType::RD_KAFKA_RESOURCE_BROKER,
                ),
            };
            native_configs.push(unsafe {
                NativeConfigResource::from_ptr(rdsys::rd_kafka_ConfigResource_new(
                    typ,
                    name.as_ptr(),
                ))
            });
        }
        let (native_opts, rx) = opts.to_native(self.client.native_ptr(), &mut err_buf)?;
        unsafe {
            rdsys::rd_kafka_DescribeConfigs(
                self.client.native_ptr(),
                native_configs.as_c_array(),
                native_configs.len(),
                native_opts.ptr(),
                self.queue.ptr(),
            );
        }
        Ok(rx)
    }

    /// Sets configuration parameters for the specified resources.
    ///
    /// Note that while the API supports altering multiple resources at once, it
    /// is not transactional. Alteration of some resources may succeed while
    /// others fail. Be sure to check the result of each individual operation.
    pub fn alter_configs<'a, I>(
        &self,
        configs: I,
        opts: &AdminOptions,
    ) -> impl Future<Item = Vec<AlterConfigsResult>, Error = KafkaError>
    where
        I: IntoIterator<Item = &'a AlterConfig<'a>>,
    {
        match self.alter_configs_inner(configs, opts) {
            Ok(rx) => Either::A(AlterConfigsFuture { rx }),
            Err(err) => Either::B(future::err(err)),
        }
    }

    fn alter_configs_inner<'a, I>(
        &self,
        configs: I,
        opts: &AdminOptions,
    ) -> KafkaResult<Oneshot<NativeEvent>>
    where
        I: IntoIterator<Item = &'a AlterConfig<'a>>,
    {
        let mut native_configs = Vec::new();
        let mut err_buf = ErrBuf::new();
        for c in configs {
            native_configs.push(c.to_native(&mut err_buf)?);
        }
        let (native_opts, rx) = opts.to_native(self.client.native_ptr(), &mut err_buf)?;
        unsafe {
            rdsys::rd_kafka_AlterConfigs(
                self.client.native_ptr(),
                native_configs.as_c_array(),
                native_configs.len(),
                native_opts.ptr(),
                self.queue.ptr(),
            );
        }
        Ok(rx)
    }
}

impl FromClientConfig for AdminClient<DefaultClientContext> {
    fn from_config(config: &ClientConfig) -> KafkaResult<AdminClient<DefaultClientContext>> {
        AdminClient::from_config_and_context(config, DefaultClientContext)
    }
}

impl<C: ClientContext> FromClientConfigAndContext<C> for AdminClient<C> {
    fn from_config_and_context(config: &ClientConfig, context: C) -> KafkaResult<AdminClient<C>> {
        let native_config = config.create_native_config()?;
        // librdkafka only provides consumer and producer types. We follow the
        // example of the Python bindings in choosing to pretend to be a
        // producer, as producer clients are allegedly more lightweight. [0]
        //
        // [0]: https://github.com/confluentinc/confluent-kafka-python/blob/bfb07dfbca47c256c840aaace83d3fe26c587360/confluent_kafka/src/Admin.c#L1492-L1493
        let client = Client::new(
            config,
            native_config,
            RDKafkaType::RD_KAFKA_PRODUCER,
            context,
        )?;
        let queue = Arc::new(client.new_native_queue());
        let should_stop = Arc::new(AtomicBool::new(false));
        let handle = start_poll_thread(queue.clone(), should_stop.clone());
        Ok(AdminClient {
            client,
            queue,
            should_stop,
            handle: Some(handle),
        })
    }
}

impl<C: ClientContext> Drop for AdminClient<C> {
    fn drop(&mut self) {
        trace!("Stopping polling");
        self.should_stop.store(true, Ordering::Relaxed);
        trace!("Waiting for polling thread termination");
        match self.handle.take().unwrap().join() {
            Ok(()) => trace!("Polling stopped"),
            Err(e) => warn!("Failure while terminating thread: {:?}", e),
        };
    }
}

fn start_poll_thread(queue: Arc<NativeQueue>, should_stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("admin client polling thread".into())
        .spawn(move || {
            trace!("Admin polling thread loop started");
            loop {
                let event = queue.poll(Duration::from_millis(100));
                if event.is_null() {
                    if should_stop.load(Ordering::Relaxed) {
                        // We received nothing and the thread should stop, so
                        // break the loop.
                        break;
                    }
                    continue;
                }
                let event = unsafe { NativeEvent::from_ptr(event) };
                let tx: Box<Complete<NativeEvent>> =
                    unsafe { IntoOpaque::from_ptr(rdsys::rd_kafka_event_opaque(event.ptr())) };
                let _ = tx.send(event);
            }
            trace!("Admin polling thread loop terminated");
        })
        .expect("Failed to start polling thread")
}

struct NativeEvent {
    ptr: *mut RDKafkaEvent,
}

impl NativeEvent {
    unsafe fn from_ptr(ptr: *mut RDKafkaEvent) -> NativeEvent {
        NativeEvent { ptr }
    }

    fn ptr(&self) -> *mut RDKafkaEvent {
        self.ptr
    }

    fn check_error(&self) -> KafkaResult<()> {
        let err = unsafe { rdsys::rd_kafka_event_error(self.ptr) };
        if err.is_error() {
            Err(KafkaError::AdminOp(err.into()))
        } else {
            Ok(())
        }
    }
}

impl Drop for NativeEvent {
    fn drop(&mut self) {
        trace!("Destroying event: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_event_destroy(self.ptr);
        }
        trace!("Event destroyed: {:?}", self.ptr);
    }
}

unsafe impl Sync for NativeEvent {}
unsafe impl Send for NativeEvent {}

//
// ********** ADMIN OPTIONS **********
//

/// Options for an admin API request.
pub struct AdminOptions {
    request_timeout: Option<Duration>,
    operation_timeout: Option<Duration>,
    validate_only: bool,
    broker_id: Option<i32>,
}

impl AdminOptions {
    /// Creates a new `AdminOptions`.
    pub fn new() -> AdminOptions {
        AdminOptions {
            request_timeout: None,
            operation_timeout: None,
            validate_only: false,
            broker_id: None,
        }
    }

    /// Sets the overall request timeout, including broker lookup, request
    /// transmission, operation time on broker, and response.
    ///
    /// Defaults to the `socket.timeout.ms` configuration parameter.
    pub fn request_timeout<T: Into<Option<Duration>>>(mut self, timeout: T) -> Self {
        self.request_timeout = timeout.into();
        self
    }

    /// Sets the broker's operation timeout, such as the timeout for
    /// CreateTopics to complete the creation of topics on the controller before
    /// returning a result to the application.
    ///
    /// If unset (the default), the API calls will return immediately after
    /// triggering the operation.
    ///
    /// Only the CreateTopics, DeleteTopics, and CreatePartitions API calls
    /// respect this option.
    pub fn operation_timeout<T: Into<Option<Duration>>>(mut self, timeout: T) -> Self {
        self.operation_timeout = timeout.into();
        self
    }

    /// Tells the broker to only validate the request, without performing the
    /// requested operation.
    ///
    /// Defaults to false.
    pub fn validate_only(mut self, validate_only: bool) -> Self {
        self.validate_only = validate_only;
        self
    }

    /// Override what broker the admin request will be sent to.
    ///
    /// By default, a reasonable broker will be selected automatically. See the
    /// librdkafka docs on `rd_kafka_AdminOptions_set_broker` for details.
    pub fn broker_id<T: Into<Option<i32>>>(mut self, broker_id: T) -> Self {
        self.broker_id = broker_id.into();
        self
    }

    fn to_native(
        &self,
        client: *mut RDKafka,
        err_buf: &mut ErrBuf,
    ) -> KafkaResult<(NativeAdminOptions, Oneshot<NativeEvent>)> {
        let native_opts = unsafe {
            NativeAdminOptions::from_ptr(rdsys::rd_kafka_AdminOptions_new(
                client,
                RDKafkaAdminOp::RD_KAFKA_ADMIN_OP_ANY,
            ))
        };

        if let Some(timeout) = self.request_timeout {
            let res = unsafe {
                rdsys::rd_kafka_AdminOptions_set_request_timeout(
                    native_opts.ptr(),
                    timeout_to_ms(timeout),
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }

        if let Some(timeout) = self.operation_timeout {
            let res = unsafe {
                rdsys::rd_kafka_AdminOptions_set_operation_timeout(
                    native_opts.ptr(),
                    timeout_to_ms(timeout),
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }

        if self.validate_only {
            let res = unsafe {
                rdsys::rd_kafka_AdminOptions_set_validate_only(
                    native_opts.ptr(),
                    1, // true
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }

        if let Some(broker_id) = self.broker_id {
            let res = unsafe {
                rdsys::rd_kafka_AdminOptions_set_broker(
                    native_opts.ptr(),
                    broker_id,
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }

        let (tx, rx) = futures::oneshot();
        let tx = Box::new(tx);
        unsafe {
            rdsys::rd_kafka_AdminOptions_set_opaque(native_opts.ptr, IntoOpaque::as_ptr(&tx))
        };
        mem::forget(tx);

        Ok((native_opts, rx))
    }
}

struct NativeAdminOptions {
    ptr: *mut RDKafkaAdminOptions,
}

impl NativeAdminOptions {
    unsafe fn from_ptr(ptr: *mut RDKafkaAdminOptions) -> NativeAdminOptions {
        NativeAdminOptions { ptr }
    }

    fn ptr(&self) -> *mut RDKafkaAdminOptions {
        self.ptr
    }
}

impl Drop for NativeAdminOptions {
    fn drop(&mut self) {
        trace!("Destroying admin options: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_AdminOptions_destroy(self.ptr);
        }
        trace!("Admin options destroyed: {:?}", self.ptr);
    }
}

fn check_rdkafka_invalid_arg(res: RDKafkaRespErr, err_buf: &ErrBuf) -> KafkaResult<()> {
    match res.into() {
        RDKafkaError::NoError => Ok(()),
        RDKafkaError::InvalidArgument => {
            let msg = if err_buf.len() == 0 {
                "invalid argument".into()
            } else {
                err_buf.to_string()
            };
            Err(KafkaError::AdminOpCreation(msg))
        }
        res => Err(KafkaError::AdminOpCreation(format!(
            "setting admin options returned unexpected error code {}",
            res
        ))),
    }
}

//
// ********** RESPONSE HANDLING **********
//

/// The result of an individual CreateTopic, DeleteTopic, or
/// CreatePartition operation.
pub type TopicResult = Result<String, (String, RDKafkaError)>;

fn build_topic_results(topics: *const *const RDKafkaTopicResult, n: usize) -> Vec<TopicResult> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let topic = unsafe { *topics.offset(i as isize) };
        let name = unsafe { cstr_to_owned(rdsys::rd_kafka_topic_result_name(topic)) };
        let err = unsafe { rdsys::rd_kafka_topic_result_error(topic) };
        if err.is_error() {
            out.push(Err((name, err.into())));
        } else {
            out.push(Ok(name));
        }
    }
    out
}

//
// Create topic handling
//

/// Configuration for a CreateTopic operation.
#[derive(Debug)]
pub struct NewTopic<'a> {
    /// The name of the new topic.
    pub name: &'a str,
    /// The initial number of partitions.
    pub num_partitions: i32,
    /// The initial replication configuration.
    pub replication: TopicReplication<'a>,
    /// The initial configuration parameters for the topic.
    pub config: Vec<(&'a str, &'a str)>,
}

impl<'a> NewTopic<'a> {
    /// Creates a new `NewTopic`.
    pub fn new(
        name: &'a str,
        num_partitions: i32,
        replication: TopicReplication<'a>,
    ) -> NewTopic<'a> {
        NewTopic {
            name,
            num_partitions,
            replication,
            config: Vec::new(),
        }
    }

    /// Sets a new parameter in the initial topic configuration.
    pub fn set(mut self, key: &'a str, value: &'a str) -> NewTopic<'a> {
        self.config.push((key, value));
        self
    }

    fn to_native(&self, err_buf: &mut ErrBuf) -> KafkaResult<NativeNewTopic> {
        let name = CString::new(self.name)?;
        let repl = match self.replication {
            TopicReplication::Fixed(n) => n,
            TopicReplication::Variable(partitions) => {
                if partitions.len() as i32 != self.num_partitions {
                    return Err(KafkaError::AdminOpCreation(format!(
                        "replication configuration for topic '{}' assigns {} partition(s), \
                         which does not match the specified number of partitions ({})",
                        self.name,
                        partitions.len(),
                        self.num_partitions,
                    )));
                }
                -1
            }
        };
        let topic = unsafe {
            rdsys::rd_kafka_NewTopic_new(
                name.as_ptr(),
                self.num_partitions,
                repl,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if topic.is_null() {
            return Err(KafkaError::AdminOpCreation(err_buf.to_string()));
        }
        // N.B.: we wrap topic immediately, so that it is destroyed via the
        // NativeNewTopic's Drop implementation if replica assignment or config
        // installation fails.
        let topic = unsafe { NativeNewTopic::from_ptr(topic) };
        if let TopicReplication::Variable(assignment) = self.replication {
            for (partition_id, broker_ids) in assignment.into_iter().enumerate() {
                let res = unsafe {
                    rdsys::rd_kafka_NewTopic_set_replica_assignment(
                        topic.ptr(),
                        partition_id as i32,
                        broker_ids.as_ptr() as *mut i32,
                        broker_ids.len(),
                        err_buf.as_mut_ptr(),
                        err_buf.len(),
                    )
                };
                check_rdkafka_invalid_arg(res, err_buf)?;
            }
        }
        for (key, val) in &self.config {
            let key_c = CString::new(*key)?;
            let val_c = CString::new(*val)?;
            let res = unsafe {
                rdsys::rd_kafka_NewTopic_set_config(topic.ptr(), key_c.as_ptr(), val_c.as_ptr())
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }
        Ok(topic)
    }
}

/// An assignment of partitions to replicas.
///
/// Each element in the outer slice corresponds to the partition with that
/// index. The inner slice specifies the broker IDs to which replicas of that
/// partition should be assigned.
pub type PartitionAssignment<'a> = &'a [&'a [i32]];

/// Replication configuration for a new topic.
#[derive(Debug)]
pub enum TopicReplication<'a> {
    /// All partitions should use the same fixed replication factor.
    Fixed(i32),
    /// Each partition should use the replica assignment from
    /// `PartitionAssignment`.
    Variable(PartitionAssignment<'a>),
}

#[repr(transparent)]
struct NativeNewTopic {
    ptr: *mut RDKafkaNewTopic,
}

impl NativeNewTopic {
    unsafe fn from_ptr(ptr: *mut RDKafkaNewTopic) -> NativeNewTopic {
        NativeNewTopic { ptr }
    }
}

impl WrappedCPointer for NativeNewTopic {
    type Target = RDKafkaNewTopic;

    fn ptr(&self) -> *mut RDKafkaNewTopic {
        self.ptr
    }
}

impl Drop for NativeNewTopic {
    fn drop(&mut self) {
        trace!("Destroying new topic: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_NewTopic_destroy(self.ptr);
        }
        trace!("New topic destroyed: {:?}", self.ptr);
    }
}

struct CreateTopicsFuture {
    rx: Oneshot<NativeEvent>,
}

impl Future for CreateTopicsFuture {
    type Item = Vec<TopicResult>;
    type Error = KafkaError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(event)) => {
                event.check_error()?;
                let res = unsafe { rdsys::rd_kafka_event_CreateTopics_result(event.ptr()) };
                if res.is_null() {
                    let typ = unsafe { rdsys::rd_kafka_event_type(event.ptr()) };
                    return Err(KafkaError::AdminOpCreation(format!(
                        "create topics request received response of incorrect type ({})",
                        typ
                    )));
                }
                let mut n = 0;
                let topics = unsafe { rdsys::rd_kafka_CreateTopics_result_topics(res, &mut n) };
                Ok(Async::Ready(build_topic_results(topics, n)))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(Canceled) => Err(KafkaError::Canceled),
        }
    }
}

//
// Delete topic handling
//

#[repr(transparent)]
struct NativeDeleteTopic {
    ptr: *mut RDKafkaDeleteTopic,
}

impl NativeDeleteTopic {
    unsafe fn from_ptr(ptr: *mut RDKafkaDeleteTopic) -> NativeDeleteTopic {
        NativeDeleteTopic { ptr }
    }
}

impl WrappedCPointer for NativeDeleteTopic {
    type Target = RDKafkaDeleteTopic;

    fn ptr(&self) -> *mut RDKafkaDeleteTopic {
        self.ptr
    }
}

impl Drop for NativeDeleteTopic {
    fn drop(&mut self) {
        trace!("Destroying delete topic: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_DeleteTopic_destroy(self.ptr);
        }
        trace!("Delete topic destroyed: {:?}", self.ptr);
    }
}

struct DeleteTopicsFuture {
    rx: Oneshot<NativeEvent>,
}

impl Future for DeleteTopicsFuture {
    type Item = Vec<TopicResult>;
    type Error = KafkaError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(event)) => {
                event.check_error()?;
                let res = unsafe { rdsys::rd_kafka_event_DeleteTopics_result(event.ptr()) };
                if res.is_null() {
                    let typ = unsafe { rdsys::rd_kafka_event_type(event.ptr()) };
                    return Err(KafkaError::AdminOpCreation(format!(
                        "delete topics request received response of incorrect type ({})",
                        typ
                    )));
                }
                let mut n = 0;
                let topics = unsafe { rdsys::rd_kafka_DeleteTopics_result_topics(res, &mut n) };
                Ok(Async::Ready(build_topic_results(topics, n)))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(Canceled) => Err(KafkaError::Canceled),
        }
    }
}

//
// Create partitions handling
//

/// Configuration for a CreatePartitions operation.
pub struct NewPartitions<'a> {
    /// The name of the topic to which partitions should be added.
    pub topic_name: &'a str,
    /// The total number of partitions after the operation completes.
    pub new_partition_count: usize,
    /// The replica assignments for the new partitions.
    pub assignment: Option<PartitionAssignment<'a>>,
}

impl<'a> NewPartitions<'a> {
    /// Creates a new `NewPartitions`.
    pub fn new(topic_name: &'a str, new_partition_count: usize) -> NewPartitions<'a> {
        NewPartitions {
            topic_name,
            new_partition_count,
            assignment: None,
        }
    }

    /// Sets the partition replica assignment for the new partitions. Only
    /// assignments for newly created replicas should be included.
    pub fn assign(mut self, assignment: PartitionAssignment<'a>) -> NewPartitions {
        self.assignment = Some(assignment);
        self
    }

    fn to_native(&self, err_buf: &mut ErrBuf) -> KafkaResult<NativeNewPartitions> {
        let name = CString::new(self.topic_name)?;
        if let Some(assignment) = self.assignment {
            // If assignment contains more than self.new_partition_count
            // entries, we'll trip an assertion in librdkafka that crashes the
            // process. Note that this check isn't a guarantee that the
            // partition assignment is valid, since the assignment should only
            // contain entries for the *new* partitions added, and not any
            // existing partitions, but we can let the server handle that
            // validation--we just need to make sure not to crash librdkafka.
            if assignment.len() > self.new_partition_count {
                return Err(KafkaError::AdminOpCreation(format!(
                    "partition assignment for topic '{}' assigns {} partition(s), \
                     which is more than the requested total number of partitions ({})",
                    self.topic_name,
                    assignment.len(),
                    self.new_partition_count,
                )));
            }
        }
        let partitions = unsafe {
            rdsys::rd_kafka_NewPartitions_new(
                name.as_ptr(),
                self.new_partition_count,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if partitions.is_null() {
            return Err(KafkaError::AdminOpCreation(err_buf.to_string()));
        }
        // N.B.: we wrap partition immediately, so that it is destroyed via
        // NativeNewPartitions's Drop implementation if replica assignment or
        // config installation fails.
        let partitions = unsafe { NativeNewPartitions::from_ptr(partitions) };
        if let Some(assignment) = self.assignment {
            for (partition_id, broker_ids) in assignment.into_iter().enumerate() {
                let res = unsafe {
                    rdsys::rd_kafka_NewPartitions_set_replica_assignment(
                        partitions.ptr(),
                        partition_id as i32,
                        broker_ids.as_ptr() as *mut i32,
                        broker_ids.len(),
                        err_buf.as_mut_ptr(),
                        err_buf.len(),
                    )
                };
                check_rdkafka_invalid_arg(res, err_buf)?;
            }
        }
        Ok(partitions)
    }
}

#[repr(transparent)]
struct NativeNewPartitions {
    ptr: *mut RDKafkaNewPartitions,
}

impl NativeNewPartitions {
    unsafe fn from_ptr(ptr: *mut RDKafkaNewPartitions) -> NativeNewPartitions {
        NativeNewPartitions { ptr }
    }
}

impl WrappedCPointer for NativeNewPartitions {
    type Target = RDKafkaNewPartitions;

    fn ptr(&self) -> *mut RDKafkaNewPartitions {
        self.ptr
    }
}

impl Drop for NativeNewPartitions {
    fn drop(&mut self) {
        trace!("Destroying new partitions: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_NewPartitions_destroy(self.ptr);
        }
        trace!("New partitions destroyed: {:?}", self.ptr);
    }
}

struct CreatePartitionsFuture {
    rx: Oneshot<NativeEvent>,
}

impl Future for CreatePartitionsFuture {
    type Item = Vec<TopicResult>;
    type Error = KafkaError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(event)) => {
                event.check_error()?;
                let res = unsafe { rdsys::rd_kafka_event_CreatePartitions_result(event.ptr()) };
                if res.is_null() {
                    let typ = unsafe { rdsys::rd_kafka_event_type(event.ptr()) };
                    return Err(KafkaError::AdminOpCreation(format!(
                        "create partitions request received response of incorrect type ({})",
                        typ
                    )));
                }
                let mut n = 0;
                let topics = unsafe { rdsys::rd_kafka_CreatePartitions_result_topics(res, &mut n) };
                Ok(Async::Ready(build_topic_results(topics, n)))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(Canceled) => Err(KafkaError::Canceled),
        }
    }
}

//
// Describe configs handling
//

/// The result of an individual DescribeConfig operation.
pub type ConfigResourceResult = Result<ConfigResource, RDKafkaError>;

/// Specification of a configurable resource.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ResourceSpecifier<'a> {
    /// A topic resource, identified by its name.
    Topic(&'a str),
    /// A group resource, identified by its ID.
    Group(&'a str),
    /// A broker resource, identified by its ID.
    Broker(i32),
}

/// A `ResourceSpecifier` that owns its data.
#[derive(Debug, Eq, PartialEq)]
pub enum OwnedResourceSpecifier {
    /// A topic resource, identified by its name.
    Topic(String),
    /// A group resource, identified by its ID.
    Group(String),
    /// A broker resource, identified by its ID.
    Broker(i32),
}

/// The source of a configuration entry.
#[derive(Debug, Eq, PartialEq)]
pub enum ConfigSource {
    /// Unknown. Note that Kafka brokers before v1.1.0 do not reliably provide
    /// configuration source information.
    Unknown,
    /// A dynamic topic configuration.
    DynamicTopic,
    /// A dynamic broker configuration.
    DynamicBroker,
    /// The default dynamic broker configuration.
    DynamicDefaultBroker,
    /// The static broker configuration.
    StaticBroker,
    /// The hardcoded default configuration.
    Default,
}

/// An individual configuration parameter for a `ConfigResource`.
#[derive(Debug, Eq, PartialEq)]
pub struct ConfigEntry {
    /// The name of the configuration parameter.
    pub name: String,
    /// The value of the configuration parameter.
    pub value: Option<String>,
    /// The source of the configuration parameter.
    pub source: ConfigSource,
    /// Whether the configuration parameter is read only.
    pub is_read_only: bool,
    /// Whether the configuration parameter currently has the default value.
    pub is_default: bool,
    /// Whether the configuration parameter contains sensitive data.
    pub is_sensitive: bool,
}

/// A configurable resource and its current configuration values.
#[derive(Debug)]
pub struct ConfigResource {
    /// Identifies the resource.
    pub specifier: OwnedResourceSpecifier,
    /// The current configuration parameters.
    pub entries: Vec<ConfigEntry>,
}

impl ConfigResource {
    /// Builds a `HashMap` of configuration entries, keyed by configuration
    /// entry name.
    pub fn entry_map(&self) -> HashMap<&str, &ConfigEntry> {
        self.entries.iter().map(|e| (&*e.name, e)).collect()
    }

    /// Searches the configuration entries to find the named parameter.
    ///
    /// For more efficient lookups, use `entry_map` to build a `HashMap`
    /// instead.
    pub fn get(&self, name: &str) -> Option<&ConfigEntry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

#[repr(transparent)]
struct NativeConfigResource {
    ptr: *mut RDKafkaConfigResource,
}

impl NativeConfigResource {
    unsafe fn from_ptr(ptr: *mut RDKafkaConfigResource) -> NativeConfigResource {
        NativeConfigResource { ptr }
    }
}

impl WrappedCPointer for NativeConfigResource {
    type Target = RDKafkaConfigResource;

    fn ptr(&self) -> *mut RDKafkaConfigResource {
        self.ptr
    }
}

impl Drop for NativeConfigResource {
    fn drop(&mut self) {
        trace!("Destroying config resource: {:?}", self.ptr);
        unsafe {
            rdsys::rd_kafka_ConfigResource_destroy(self.ptr);
        }
        trace!("Config resource destroyed: {:?}", self.ptr);
    }
}

fn extract_config_specifier(
    resource: *const RDKafkaConfigResource,
) -> KafkaResult<OwnedResourceSpecifier> {
    let typ = unsafe { rdsys::rd_kafka_ConfigResource_type(resource) };
    match typ {
        RDKafkaResourceType::RD_KAFKA_RESOURCE_TOPIC => {
            let name = unsafe { cstr_to_owned(rdsys::rd_kafka_ConfigResource_name(resource)) };
            Ok(OwnedResourceSpecifier::Topic(name))
        }
        RDKafkaResourceType::RD_KAFKA_RESOURCE_GROUP => {
            let name = unsafe { cstr_to_owned(rdsys::rd_kafka_ConfigResource_name(resource)) };
            Ok(OwnedResourceSpecifier::Group(name))
        }
        RDKafkaResourceType::RD_KAFKA_RESOURCE_BROKER => {
            let name = unsafe { CStr::from_ptr(rdsys::rd_kafka_ConfigResource_name(resource)) }
                .to_string_lossy();
            match name.parse::<i32>() {
                Ok(id) => Ok(OwnedResourceSpecifier::Broker(id)),
                Err(_) => Err(KafkaError::AdminOpCreation(format!(
                    "bogus broker ID in kafka response: {}",
                    name
                ))),
            }
        }
        _ => Err(KafkaError::AdminOpCreation(format!(
            "bogus resource type in kafka response: {:?}",
            typ
        ))),
    }
}

fn extract_config_source(config_source: RDKafkaConfigSource) -> KafkaResult<ConfigSource> {
    match config_source {
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_UNKNOWN_CONFIG => Ok(ConfigSource::Unknown),
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_DYNAMIC_TOPIC_CONFIG => {
            Ok(ConfigSource::DynamicTopic)
        }
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_DYNAMIC_BROKER_CONFIG => {
            Ok(ConfigSource::DynamicBroker)
        }
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_DYNAMIC_DEFAULT_BROKER_CONFIG => {
            Ok(ConfigSource::DynamicDefaultBroker)
        }
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_STATIC_BROKER_CONFIG => {
            Ok(ConfigSource::StaticBroker)
        }
        RDKafkaConfigSource::RD_KAFKA_CONFIG_SOURCE_DEFAULT_CONFIG => Ok(ConfigSource::Default),
        _ => Err(KafkaError::AdminOpCreation(format!(
            "bogus config source type in kafka response: {:?}",
            config_source,
        ))),
    }
}

struct DescribeConfigsFuture {
    rx: Oneshot<NativeEvent>,
}

impl Future for DescribeConfigsFuture {
    type Item = Vec<ConfigResourceResult>;
    type Error = KafkaError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(event)) => {
                event.check_error()?;
                let res = unsafe { rdsys::rd_kafka_event_DescribeConfigs_result(event.ptr()) };
                if res.is_null() {
                    let typ = unsafe { rdsys::rd_kafka_event_type(event.ptr()) };
                    return Err(KafkaError::AdminOpCreation(format!(
                        "describe configs request received response of incorrect type ({})",
                        typ
                    )));
                }
                let mut n = 0;
                let resources =
                    unsafe { rdsys::rd_kafka_DescribeConfigs_result_resources(res, &mut n) };
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let resource = unsafe { *resources.offset(i as isize) };
                    let specifier = extract_config_specifier(resource)?;
                    let mut entries_out = Vec::new();
                    let mut n = 0;
                    let entries =
                        unsafe { rdsys::rd_kafka_ConfigResource_configs(resource, &mut n) };
                    for j in 0..n {
                        let entry = unsafe { *entries.offset(j as isize) };
                        let name =
                            unsafe { cstr_to_owned(rdsys::rd_kafka_ConfigEntry_name(entry)) };
                        let value = unsafe {
                            let value = rdsys::rd_kafka_ConfigEntry_value(entry);
                            if value.is_null() {
                                None
                            } else {
                                Some(cstr_to_owned(value))
                            }
                        };
                        entries_out.push(ConfigEntry {
                            name,
                            value,
                            source: extract_config_source(unsafe {
                                rdsys::rd_kafka_ConfigEntry_source(entry)
                            })?,
                            is_read_only: unsafe {
                                rdsys::rd_kafka_ConfigEntry_is_read_only(entry)
                            } != 0,
                            is_default: unsafe { rdsys::rd_kafka_ConfigEntry_is_default(entry) }
                                != 0,
                            is_sensitive: unsafe {
                                rdsys::rd_kafka_ConfigEntry_is_sensitive(entry)
                            } != 0,
                        });
                    }
                    out.push(Ok(ConfigResource {
                        specifier: specifier,
                        entries: entries_out,
                    }))
                }
                Ok(Async::Ready(out))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(Canceled) => Err(KafkaError::Canceled),
        }
    }
}

//
// Alter configs handling
//

/// The result of an individual AlterConfig operation.
pub type AlterConfigsResult =
    Result<OwnedResourceSpecifier, (OwnedResourceSpecifier, RDKafkaError)>;

/// Configuration for an AlterConfig operation.
pub struct AlterConfig<'a> {
    /// Identifies the resource to be altered.
    pub specifier: ResourceSpecifier<'a>,
    /// The configuration parameters to be updated.
    pub entries: HashMap<&'a str, &'a str>,
}

impl<'a> AlterConfig<'a> {
    /// Creates a new `AlterConfig`.
    pub fn new(specifier: ResourceSpecifier) -> AlterConfig {
        AlterConfig {
            specifier,
            entries: HashMap::new(),
        }
    }

    /// Sets the configuration parameter named `key` to the specified `value`.
    pub fn set(mut self, key: &'a str, value: &'a str) -> AlterConfig<'a> {
        self.entries.insert(key, value);
        self
    }

    fn to_native(&self, err_buf: &mut ErrBuf) -> KafkaResult<NativeConfigResource> {
        let (name, typ) = match self.specifier {
            ResourceSpecifier::Topic(name) => (
                CString::new(name)?,
                RDKafkaResourceType::RD_KAFKA_RESOURCE_TOPIC,
            ),
            ResourceSpecifier::Group(name) => (
                CString::new(name)?,
                RDKafkaResourceType::RD_KAFKA_RESOURCE_GROUP,
            ),
            ResourceSpecifier::Broker(id) => (
                CString::new(format!("{}", id))?,
                RDKafkaResourceType::RD_KAFKA_RESOURCE_BROKER,
            ),
        };
        // N.B.: we wrap config immediately, so that it is destroyed via the
        // NativeNewTopic's Drop implementation if config installation fails.
        let config = unsafe {
            NativeConfigResource::from_ptr(rdsys::rd_kafka_ConfigResource_new(typ, name.as_ptr()))
        };
        for (key, val) in &self.entries {
            let key_c = CString::new(*key)?;
            let val_c = CString::new(*val)?;
            let res = unsafe {
                rdsys::rd_kafka_ConfigResource_set_config(
                    config.ptr(),
                    key_c.as_ptr(),
                    val_c.as_ptr(),
                )
            };
            check_rdkafka_invalid_arg(res, err_buf)?;
        }
        Ok(config)
    }
}

struct AlterConfigsFuture {
    rx: Oneshot<NativeEvent>,
}

impl Future for AlterConfigsFuture {
    type Item = Vec<AlterConfigsResult>;
    type Error = KafkaError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(event)) => {
                event.check_error()?;
                let res = unsafe { rdsys::rd_kafka_event_AlterConfigs_result(event.ptr()) };
                if res.is_null() {
                    let typ = unsafe { rdsys::rd_kafka_event_type(event.ptr()) };
                    return Err(KafkaError::AdminOpCreation(format!(
                        "alter configs request received response of incorrect type ({})",
                        typ
                    )));
                }
                let mut n = 0;
                let resources =
                    unsafe { rdsys::rd_kafka_AlterConfigs_result_resources(res, &mut n) };
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let resource = unsafe { *resources.offset(i as isize) };
                    let specifier = extract_config_specifier(resource)?;
                    out.push(Ok(specifier));
                }
                Ok(Async::Ready(out))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(Canceled) => Err(KafkaError::Canceled),
        }
    }
}
