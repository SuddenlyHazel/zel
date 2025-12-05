use crate::service::{NotificationDescription, SubscriptionDescription};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use super::codegen::generate_typed_name;

/// Render all typed sinks for subscriptions
pub fn render_typed_sinks(
    subscriptions: &[SubscriptionDescription],
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let typed_sinks: Vec<_> = subscriptions
        .iter()
        .map(|s| render_typed_sink(s, trait_name))
        .collect();

    quote! {
        #(#typed_sinks)*
    }
}

/// Render a single typed sink for a subscription
pub fn render_typed_sink(sub: &SubscriptionDescription, trait_name: &syn::Ident) -> TokenStream2 {
    let method_name = &sub.signature.sig.ident;
    let sink_name = generate_typed_name(trait_name, method_name, "Sink");

    let item_type = sub
        .item_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    quote! {
        pub struct #sink_name {
            inner: zel_core::protocol::SubscriptionSink,
        }

        impl #sink_name {
            pub async fn send(&mut self, data: #item_type) -> Result<(), zel_core::protocol::SubscriptionError> {
                self.inner.send(&data).await
            }

            pub async fn close(self) -> Result<(), zel_core::protocol::SubscriptionError> {
                self.inner.close().await
            }
        }
    }
}

/// Render all typed receivers for notifications
pub fn render_typed_receivers(
    notifications: &[NotificationDescription],
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let typed_receivers: Vec<_> = notifications
        .iter()
        .map(|n| render_typed_receiver(n, trait_name))
        .collect();

    quote! {
        #(#typed_receivers)*
    }
}

/// Render a single typed receiver for a notification
pub fn render_typed_receiver(
    notif: &NotificationDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &notif.signature.sig.ident;
    let receiver_name = generate_typed_name(trait_name, method_name, "Receiver");

    let item_type = notif
        .item_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    quote! {
        pub struct #receiver_name {
            inner: tokio_util::codec::FramedRead<iroh::endpoint::RecvStream, tokio_util::codec::LengthDelimitedCodec>,
            sink: zel_core::protocol::NotificationSink,
        }

        impl #receiver_name {
            pub async fn next(&mut self) -> Option<Result<#item_type, zel_core::protocol::NotificationError>> {
                use futures::StreamExt;

                loop {
                    let bytes = match self.inner.next().await {
                        Some(Ok(b)) => b,
                        Some(Err(e)) => return Some(Err(zel_core::protocol::NotificationError::Receive(e.to_string()))),
                        None => return None,
                    };

                    let msg: zel_core::protocol::NotificationMsg = match serde_json::from_slice(&bytes) {
                        Ok(m) => m,
                        Err(e) => return Some(Err(zel_core::protocol::NotificationError::Deserialization(e.to_string()))),
                    };

                    match msg {
                        zel_core::protocol::NotificationMsg::Data(data) => {
                            let item: #item_type = match serde_json::from_slice(&data) {
                                Ok(i) => i,
                                Err(e) => return Some(Err(zel_core::protocol::NotificationError::Deserialization(e.to_string()))),
                            };

                            // Send acknowledgment
                            if let Err(e) = self.sink.ack().await {
                                return Some(Err(e));
                            }

                            return Some(Ok(item));
                        }
                        zel_core::protocol::NotificationMsg::Completed => {
                            return None;
                        }
                        zel_core::protocol::NotificationMsg::ServerShutdown => {
                            // Server is shutting down gracefully
                            return None;
                        }
                        _ => {
                            // Ignore other message types
                            continue;
                        }
                    }
                }
            }
        }
    }
}

/// Render a client-side subscription stream
pub fn render_subscription_stream(
    sub: &SubscriptionDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &sub.signature.sig.ident;
    let stream_name = generate_typed_name(trait_name, method_name, "Stream");

    let item_type = sub
        .item_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    quote! {
        pub struct #stream_name {
            inner: zel_core::protocol::SubscriptionStream,
        }

        impl futures::Stream for #stream_name {
            type Item = Result<#item_type, zel_core::protocol::ClientError>;

            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Self::Item>> {
                use futures::StreamExt;

                match std::pin::Pin::new(&mut self.inner).poll_next(cx) {
                    std::task::Poll::Ready(Some(Ok(zel_core::protocol::SubscriptionMsg::Data(data)))) => {
                        match serde_json::from_slice::<#item_type>(&data) {
                            Ok(value) => std::task::Poll::Ready(Some(Ok(value))),
                            Err(e) => std::task::Poll::Ready(Some(Err(
                                zel_core::protocol::ClientError::Serialization(e)
                            ))),
                        }
                    }
                    std::task::Poll::Ready(Some(Ok(zel_core::protocol::SubscriptionMsg::Stopped))) => {
                        std::task::Poll::Ready(None)
                    }
                    std::task::Poll::Ready(Some(Ok(zel_core::protocol::SubscriptionMsg::ServerShutdown))) => {
                        // Server is shutting down gracefully, close the stream
                        std::task::Poll::Ready(None)
                    }
                    std::task::Poll::Ready(Some(Ok(zel_core::protocol::SubscriptionMsg::Established { .. }))) => {
                        // Skip the Established message and poll again
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                    std::task::Poll::Ready(Some(Err(e))) => {
                        std::task::Poll::Ready(Some(Err(e)))
                    }
                    std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }
    }
}

/// Render a client-side notification sender
pub fn render_notification_sender(
    notif: &NotificationDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &notif.signature.sig.ident;
    let sender_name = generate_typed_name(trait_name, method_name, "Sender");

    let item_type = notif
        .item_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    quote! {
        pub struct #sender_name {
            inner: zel_core::protocol::NotificationSender,
        }

        impl #sender_name {
            pub async fn send(&mut self, data: #item_type) -> Result<(), zel_core::protocol::ClientError> {
                self.inner.send(data).await
            }

            pub async fn complete(self) -> Result<(), zel_core::protocol::ClientError> {
                self.inner.complete().await
            }
        }
    }
}
