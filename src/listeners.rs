use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::channel::mpsc::{SendError, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use futures::{Sink, Stream};

use chromiumoxide_cdp::cdp::{Event, EventKind, IntoEventKind};
use chromiumoxide_types::MethodId;

/// All the currently active listeners
#[derive(Debug, Default)]
pub struct EventListeners {
    /// Tracks the listeners for each event identified by the key
    listeners: HashMap<MethodId, Vec<EventListener>>,
    /// Wildcard listeners that receive all events regardless of type
    wildcard_listeners: Vec<EventListener>,
}

impl EventListeners {
    /// Register a subscription for a method
    pub fn add_listener(&mut self, req: EventListenerRequest) {
        let EventListenerRequest {
            listener,
            method,
            kind,
            ack,
            wildcard,
        } = req;

        let event_listener = EventListener {
            listener,
            kind,
            queued_events: Default::default(),
        };

        if wildcard {
            self.wildcard_listeners.push(event_listener);
        } else {
            let subs = self.listeners.entry(method).or_default();
            subs.push(event_listener);
        }

        // Send acknowledgment that listener was registered
        if let Some(tx) = ack {
            let _ = tx.send(());
        }
    }

    /// Queue in a event that should be send to all listeners
    pub fn start_send<T: Event>(&mut self, event: T) {
        let has_typed = self.listeners.contains_key(&T::method_id());
        let has_wildcard = !self.wildcard_listeners.is_empty();

        if !has_typed && !has_wildcard {
            return;
        }

        let event: Arc<dyn Event> = Arc::new(event);

        // Send to typed listeners
        if let Some(subscriptions) = self.listeners.get_mut(&T::method_id()) {
            subscriptions
                .iter_mut()
                .for_each(|sub| sub.start_send(Arc::clone(&event)));
        }

        // Send to wildcard listeners
        self.wildcard_listeners
            .iter_mut()
            .for_each(|sub| sub.start_send(Arc::clone(&event)));
    }

    /// Try to queue in a new custom event if a listener is registered and the
    /// converting the json value to the registered event type succeeds
    pub fn try_send_custom(
        &mut self,
        method: &str,
        val: serde_json::Value,
    ) -> serde_json::Result<()> {
        if let Some(subscriptions) = self.listeners.get_mut(method) {
            let mut event = None;
            if let Some(json_to_arc_event) = subscriptions
                .iter()
                .filter_map(|sub| {
                    if let EventKind::Custom(conv) = &sub.kind {
                        Some(conv)
                    } else {
                        None
                    }
                })
                .next()
            {
                event = Some(json_to_arc_event(val)?);
            }
            if let Some(event) = event {
                subscriptions
                    .iter_mut()
                    .filter(|sub| sub.kind.is_custom())
                    .for_each(|sub| sub.start_send(Arc::clone(&event)));
            }
        }
        Ok(())
    }

    /// Drains all queued events and does the housekeeping when the receiver
    /// part of a subscription is dropped
    pub fn poll(&mut self, cx: &mut Context<'_>) {
        // Poll typed listeners
        for subscriptions in self.listeners.values_mut() {
            for n in (0..subscriptions.len()).rev() {
                let mut sub = subscriptions.swap_remove(n);
                match sub.poll(cx) {
                    Poll::Ready(Err(err)) => {
                        if !err.is_disconnected() {
                            subscriptions.push(sub)
                        }
                    }
                    _ => subscriptions.push(sub),
                }
            }
        }

        // Poll wildcard listeners
        for n in (0..self.wildcard_listeners.len()).rev() {
            let mut sub = self.wildcard_listeners.swap_remove(n);
            match sub.poll(cx) {
                Poll::Ready(Err(err)) => {
                    if !err.is_disconnected() {
                        self.wildcard_listeners.push(sub)
                    }
                }
                _ => self.wildcard_listeners.push(sub),
            }
        }
    }
}

pub struct EventListenerRequest {
    listener: UnboundedSender<Arc<dyn Event>>,
    method: MethodId,
    kind: EventKind,
    /// Optional channel to acknowledge that the listener was registered
    ack: Option<oneshot::Sender<()>>,
    /// If true, this listener receives all events regardless of method
    wildcard: bool,
}

impl EventListenerRequest {
    pub fn new<T: IntoEventKind>(listener: UnboundedSender<Arc<dyn Event>>) -> Self {
        Self {
            listener,
            method: T::method_id(),
            kind: T::event_kind(),
            ack: None,
            wildcard: false,
        }
    }

    /// Create a new request with an acknowledgment channel
    pub fn with_ack<T: IntoEventKind>(
        listener: UnboundedSender<Arc<dyn Event>>,
        ack: oneshot::Sender<()>,
    ) -> Self {
        Self {
            listener,
            method: T::method_id(),
            kind: T::event_kind(),
            ack: Some(ack),
            wildcard: false,
        }
    }

    /// Create a wildcard listener request that receives all events
    pub fn wildcard(listener: UnboundedSender<Arc<dyn Event>>, ack: oneshot::Sender<()>) -> Self {
        Self {
            listener,
            method: "".into(), // unused for wildcard
            kind: EventKind::BuiltIn,
            ack: Some(ack),
            wildcard: true,
        }
    }
}

impl fmt::Debug for EventListenerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventListenerRequest")
            .field("method", &self.method)
            .field("kind", &self.kind)
            .finish()
    }
}

/// Represents a single event listener
pub struct EventListener {
    /// the sender half of the event channel
    listener: UnboundedSender<Arc<dyn Event>>,
    /// currently queued events
    queued_events: VecDeque<Arc<dyn Event>>,
    /// For what kind of event this event is for
    kind: EventKind,
}

impl EventListener {
    /// queue in a new event
    pub fn start_send(&mut self, event: Arc<dyn Event>) {
        self.queued_events.push_back(event)
    }

    /// Drains all queued events and begins the process of sending them to the
    /// sink.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), SendError>> {
        loop {
            match Sink::poll_ready(Pin::new(&mut self.listener), cx) {
                Poll::Ready(Ok(_)) => {}
                Poll::Ready(Err(err)) => {
                    // disconnected
                    return Poll::Ready(Err(err));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
            if let Some(event) = self.queued_events.pop_front() {
                if let Err(err) = Sink::start_send(Pin::new(&mut self.listener), event) {
                    return Poll::Ready(Err(err));
                }
            } else {
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl fmt::Debug for EventListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventListener").finish()
    }
}

/// The receiver part of an event subscription
pub struct EventStream<T: IntoEventKind> {
    events: UnboundedReceiver<Arc<dyn Event>>,
    _marker: PhantomData<T>,
}

impl<T: IntoEventKind> fmt::Debug for EventStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream").finish()
    }
}

impl<T: IntoEventKind> EventStream<T> {
    pub fn new(events: UnboundedReceiver<Arc<dyn Event>>) -> Self {
        Self {
            events,
            _marker: PhantomData,
        }
    }
}

impl<T: IntoEventKind + Unpin> Stream for EventStream<T> {
    type Item = Arc<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pin = self.get_mut();
        match Stream::poll_next(Pin::new(&mut pin.events), cx) {
            Poll::Ready(Some(event)) => {
                if let Ok(e) = event.into_any_arc().downcast() {
                    Poll::Ready(Some(e))
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A stream that receives all CDP events regardless of type.
///
/// Unlike `EventStream<T>` which filters for a specific event type,
/// `AnyEventStream` yields every event as `Arc<dyn Event>`.
/// To identify and work with specific event types, downcast using
/// `event.into_any_arc().downcast::<T>()` where `T` is the concrete event type.
#[derive(Debug)]
pub struct AnyEventStream {
    events: UnboundedReceiver<Arc<dyn Event>>,
}

impl AnyEventStream {
    pub fn new(events: UnboundedReceiver<Arc<dyn Event>>) -> Self {
        Self { events }
    }
}

impl Stream for AnyEventStream {
    type Item = Arc<dyn Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pin = self.get_mut();
        Stream::poll_next(Pin::new(&mut pin.events), cx)
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};

    use chromiumoxide_cdp::cdp::browser_protocol::animation::EventAnimationCanceled;
    use chromiumoxide_cdp::cdp::CustomEvent;
    use chromiumoxide_types::MethodType;

    use super::*;

    #[tokio::test]
    async fn event_stream() {
        let (mut tx, rx) = futures::channel::mpsc::unbounded();
        let mut stream = EventStream::<EventAnimationCanceled>::new(rx);

        let event = EventAnimationCanceled {
            id: "id".to_string(),
        };
        let msg: Arc<dyn Event> = Arc::new(event.clone());
        tx.send(msg).await.unwrap();
        let next = stream.next().await.unwrap();
        assert_eq!(&*next, &event);
    }

    #[tokio::test]
    async fn custom_event_stream() {
        use serde::Deserialize;

        #[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
        struct MyCustomEvent {
            name: String,
        }

        impl MethodType for MyCustomEvent {
            fn method_id() -> MethodId {
                "Custom.Event".into()
            }
        }
        impl CustomEvent for MyCustomEvent {}

        let (mut tx, rx) = futures::channel::mpsc::unbounded();
        let mut stream = EventStream::<MyCustomEvent>::new(rx);

        let event = MyCustomEvent {
            name: "my event".to_string(),
        };
        let msg: Arc<dyn Event> = Arc::new(event.clone());
        tx.send(msg).await.unwrap();
        let next = stream.next().await.unwrap();
        assert_eq!(&*next, &event);
    }

    #[tokio::test]
    async fn event_listeners() {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let mut listeners = EventListeners::default();

        let event = EventAnimationCanceled {
            id: "id".to_string(),
        };

        listeners.add_listener(EventListenerRequest {
            method: EventAnimationCanceled::method_id(),
            kind: EventAnimationCanceled::event_kind(),
            listener: tx,
            ack: None,
            wildcard: false,
        });

        listeners.start_send(event.clone());

        let mut stream = EventStream::<EventAnimationCanceled>::new(rx);

        tokio::spawn(async move {
            loop {
                std::future::poll_fn(|cx| {
                    listeners.poll(cx);
                    Poll::Pending
                })
                .await
            }
        });

        let next = stream.next().await.unwrap();
        assert_eq!(&*next, &event);
    }

    #[tokio::test]
    async fn wildcard_listeners() {
        use chromiumoxide_cdp::cdp::browser_protocol::page::EventFrameStartedLoading;

        let (typed_tx, typed_rx) = futures::channel::mpsc::unbounded();
        let (wildcard_tx, wildcard_rx) = futures::channel::mpsc::unbounded();
        let mut listeners = EventListeners::default();

        // Register a typed listener for AnimationCanceled
        listeners.add_listener(EventListenerRequest {
            method: EventAnimationCanceled::method_id(),
            kind: EventAnimationCanceled::event_kind(),
            listener: typed_tx,
            ack: None,
            wildcard: false,
        });

        // Register a wildcard listener
        listeners.add_listener(EventListenerRequest {
            method: "".into(),
            kind: EventKind::BuiltIn,
            listener: wildcard_tx,
            ack: None,
            wildcard: true,
        });

        // Send two different event types
        let animation_event = EventAnimationCanceled {
            id: "anim1".to_string(),
        };
        let frame_event = EventFrameStartedLoading {
            frame_id: "frame1".to_string().into(),
        };

        listeners.start_send(animation_event.clone());
        listeners.start_send(frame_event.clone());

        let mut typed_stream = EventStream::<EventAnimationCanceled>::new(typed_rx);
        let mut wildcard_stream = AnyEventStream::new(wildcard_rx);

        tokio::spawn(async move {
            loop {
                std::future::poll_fn(|cx| {
                    listeners.poll(cx);
                    Poll::Pending
                })
                .await
            }
        });

        // Typed stream should only receive AnimationCanceled
        let typed_event = typed_stream.next().await.unwrap();
        assert_eq!(&*typed_event, &animation_event);

        // Wildcard stream should receive both events in order
        // First event: AnimationCanceled
        let wild_event1 = wildcard_stream.next().await.unwrap();
        let downcast1: Arc<EventAnimationCanceled> = wild_event1.into_any_arc().downcast().unwrap();
        assert_eq!(&*downcast1, &animation_event);

        // Second event: FrameStartedLoading
        let wild_event2 = wildcard_stream.next().await.unwrap();
        let downcast2: Arc<EventFrameStartedLoading> = wild_event2.into_any_arc().downcast().unwrap();
        assert_eq!(&*downcast2, &frame_event);
    }
}
