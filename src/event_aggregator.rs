use std::{
    any::{type_name, Any, TypeId},
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    sync::{Mutex, RwLock},
};

use once_cell::sync::Lazy;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::loggingchan::*;

pub static EVENT_AGGREGATOR: Lazy<EventAggregator> = Lazy::new(|| EventAggregator::default());

thread_local! {
    static THREAD_SENDERS: RwLock<HashMap<TypeId, Box<dyn Any + Send>>> = RwLock::new(HashMap::new());
}

pub struct EventAggregator {
    parent_senders: RwLock<HashMap<TypeId, Mutex<Box<dyn Any + Send>>>>,
    unclaimed_receivers: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

impl Default for EventAggregator {
    fn default() -> Self {
        EventAggregator {
            parent_senders: RwLock::new(HashMap::new()),
            unclaimed_receivers: RwLock::new(HashMap::new()),
        }
    }
}

impl EventAggregator {
    fn get_sender<T: Any + Clone + Debug + Send>(&self) -> LoggingTx<T> {
        match self
            .parent_senders
            .write()
            .unwrap()
            .entry(TypeId::of::<T>())
        {
            Entry::Occupied(entry) => {
                let sender = entry.get().lock().unwrap();
                sender.downcast_ref::<LoggingTx<T>>().unwrap().clone()
            }
            Entry::Vacant(entry) => {
                let (sender, receiver) = unbounded_channel();
                let logging_tx = LoggingTx::attach(sender, type_name::<T>().to_owned());
                entry.insert(Mutex::new(Box::new(logging_tx.clone())));
                self.unclaimed_receivers
                    .write()
                    .unwrap()
                    .insert(TypeId::of::<T>(), Box::new(receiver));
                logging_tx
            }
        }
    }

    pub fn send<T: Any + Clone + Debug + Send>(&self, event: T) {
        let sender = self.get_sender::<T>();
        sender.send(event).unwrap();
    }

    pub fn register_event<T: Any + Clone + Debug + Send>(&self) -> UnboundedReceiver<T> {
        let type_id = TypeId::of::<T>();

        if let Some(receiver) = self.unclaimed_receivers.write().unwrap().remove(&type_id) {
            *receiver.downcast::<UnboundedReceiver<T>>().unwrap()
        } else {
            let (sender, receiver) = unbounded_channel();
            let logging_sender = LoggingTx::attach(sender, type_name::<T>().to_owned());

            match self.parent_senders.write().unwrap().entry(type_id) {
                Entry::Occupied(_) => panic!("EventAggregator: type already registered"),
                Entry::Vacant(entry) => {
                    entry.insert(Mutex::new(Box::new(logging_sender)));
                }
            }

            receiver
        }
    }
}
