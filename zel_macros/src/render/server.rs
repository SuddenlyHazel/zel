use crate::service::{
    MethodDescription, NotificationDescription, ServiceDescription, StreamDescription,
    SubscriptionDescription,
};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use super::codegen::{generate_param_deserialization, generate_typed_name};

/// Render the server trait with all methods, subscriptions, notifications, and streams
pub fn render_server_trait(service: &ServiceDescription) -> TokenStream2 {
    let trait_name = &service.trait_ident;
    let server_name = quote::format_ident!("{}Server", trait_name);

    // Render method signatures with injected RequestContext
    let methods: Vec<_> = service
        .methods
        .iter()
        .map(|m| {
            let mut sig = m.signature.sig.clone();
            let attrs = &m.signature.attrs;

            // Insert RequestContext parameter as first argument (after &self)
            let ctx_param: syn::FnArg = syn::parse_quote! {
                ctx: zel_core::protocol::RequestContext
            };
            sig.inputs.insert(1, ctx_param);

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Render subscription signatures with injected RequestContext and TYPED sink parameter
    let subscriptions: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| {
            let mut sig = s.signature.sig.clone();
            let attrs = &s.signature.attrs;

            // Generate typed sink name
            let method_name = &s.signature.sig.ident;
            let sink_type = generate_typed_name(trait_name, method_name, "Sink");

            // Insert RequestContext parameter as first argument (after &self)
            let ctx_param: syn::FnArg = syn::parse_quote! {
                ctx: zel_core::protocol::RequestContext
            };
            sig.inputs.insert(1, ctx_param);

            // Insert TYPED sink parameter as second argument (after &self and ctx)
            let sink_param: syn::FnArg = syn::parse_quote! {
                sink: #sink_type
            };
            sig.inputs.insert(2, sink_param);

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Render notification signatures with injected RequestContext and TYPED receiver parameter
    let notifications: Vec<_> = service
        .notifications
        .iter()
        .map(|n| {
            let mut sig = n.signature.sig.clone();
            let attrs = &n.signature.attrs;

            // Generate typed receiver name
            let method_name = &n.signature.sig.ident;
            let receiver_type = generate_typed_name(trait_name, method_name, "Receiver");

            // Insert RequestContext parameter as first argument (after &self)
            let ctx_param: syn::FnArg = syn::parse_quote! {
                ctx: zel_core::protocol::RequestContext
            };
            sig.inputs.insert(1, ctx_param);

            // Insert TYPED receiver parameter as second argument (after &self and ctx)
            let receiver_param: syn::FnArg = syn::parse_quote! {
                receiver: #receiver_type
            };
            sig.inputs.insert(2, receiver_param);

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Render stream signatures with injected RequestContext and raw SendStream/RecvStream
    let streams: Vec<_> = service
        .streams
        .iter()
        .map(|s| {
            let mut sig = s.signature.sig.clone();
            let attrs = &s.signature.attrs;

            // Insert RequestContext parameter as first argument (after &self)
            let ctx_param: syn::FnArg = syn::parse_quote! {
                ctx: zel_core::protocol::RequestContext
            };
            sig.inputs.insert(1, ctx_param);

            // Insert SendStream parameter as second argument
            let send_param: syn::FnArg = syn::parse_quote! {
                send: iroh::endpoint::SendStream
            };
            sig.inputs.insert(2, send_param);

            // Insert RecvStream parameter as third argument
            let recv_param: syn::FnArg = syn::parse_quote! {
                recv: iroh::endpoint::RecvStream
            };
            sig.inputs.insert(3, recv_param);

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Generate the register_service method body
    let register_service_body = render_register_service_body(service);

    quote! {
        #[async_trait::async_trait]
        pub trait #server_name: Clone + Send + Sync + 'static {
            #(#methods)*
            #(#subscriptions)*
            #(#notifications)*
            #(#streams)*

            /// Register this service implementation with an RpcServerBuilder
            fn register_service(
                self,
                builder: zel_core::protocol::RpcServerBuilder<'static>
            ) -> zel_core::protocol::RpcServerBuilder<'static> {
                #register_service_body
            }
        }
    }
}

/// Render the body of the register_service method
pub fn render_register_service_body(service: &ServiceDescription) -> TokenStream2 {
    let trait_name = &service.trait_ident;
    let service_name = &service.service_name;

    let method_registrations: Vec<_> = service
        .methods
        .iter()
        .map(render_method_registration)
        .collect();

    let subscription_registrations: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| render_subscription_registration(s, trait_name))
        .collect();

    let notification_registrations: Vec<_> = service
        .notifications
        .iter()
        .map(|n| render_notification_registration(n, trait_name))
        .collect();

    let stream_registrations: Vec<_> = service
        .streams
        .iter()
        .map(render_stream_registration)
        .collect();

    quote! {
        let service_builder = builder.service(#service_name);

        #(
            let service_builder = #method_registrations;
        )*

        #(
            let service_builder = #subscription_registrations;
        )*

        #(
            let service_builder = #notification_registrations;
        )*

        #(
            let service_builder = #stream_registrations;
        )*

        service_builder.build()
    }
}

/// Render registration code for a method
pub fn render_method_registration(method: &MethodDescription) -> TokenStream2 {
    let method_name = &method.rpc_name;
    let rust_method = &method.signature.sig.ident;
    let param_idents: Vec<_> = method.params.iter().map(|p| &p.ident).collect();

    let deserialize_params = generate_param_deserialization(&method.params);

    quote! {
        service_builder.rpc_resource(
            #method_name,
            {
                let service = self.clone();
                move |ctx: zel_core::protocol::RequestContext, req: zel_core::protocol::Request| {
                    let service = service.clone();
                    Box::pin(async move {
                        // Extract body
                        let zel_core::protocol::Body::Rpc(data) = &req.body else {
                            return Err(zel_core::protocol::ResourceError::app("Expected RPC body"));
                        };

                        // Deserialize params
                        #deserialize_params

                        // Call user's method with context
                        let result = service.#rust_method(ctx, #(#param_idents),*).await?;

                        // Serialize result
                        let data = serde_json::to_vec(&result)
                            .map_err(|e| zel_core::protocol::ResourceError::SerializationError(
                                e.to_string()
                            ))?;

                        Ok(zel_core::protocol::Response {
                            data: bytes::Bytes::from(data)
                        })
                    })
                }
            }
        )
    }
}

/// Render registration code for a subscription
pub fn render_subscription_registration(
    sub: &SubscriptionDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let sub_name = &sub.rpc_name;
    let rust_method = &sub.signature.sig.ident;
    let param_idents: Vec<_> = sub.params.iter().map(|p| &p.ident).collect();

    // Generate typed sink name
    let typed_sink_name = generate_typed_name(trait_name, rust_method, "Sink");

    let deserialize_params = generate_param_deserialization(&sub.params);

    quote! {
        service_builder.subscription_resource(
            #sub_name,
            {
                let service = self.clone();
                move |
                    ctx: zel_core::protocol::RequestContext,
                    req: zel_core::protocol::Request,
                    inner_sink: tokio_util::codec::FramedWrite<
                        iroh::endpoint::SendStream,
                        tokio_util::codec::LengthDelimitedCodec
                    >
                | {
                    let service = service.clone();
                    Box::pin(async move {
                        // Extract body (for future parameter support)
                        let data = match &req.body {
                            zel_core::protocol::Body::Subscribe => &[] as &[u8],
                            zel_core::protocol::Body::Rpc(data) => data.as_ref(),
                            zel_core::protocol::Body::Stream(data) => data.as_ref(),
                            zel_core::protocol::Body::Notify(data) => data.as_ref(),
                        };

                        // Deserialize params (if any)
                        #deserialize_params

                        // Create raw SubscriptionSink and wrap in typed sink
                        let raw_sink = zel_core::protocol::SubscriptionSink::new(inner_sink);
                        let sink = #typed_sink_name { inner: raw_sink };

                        // Call user's subscription method with context and typed sink
                        service.#rust_method(ctx, sink, #(#param_idents),*).await?;

                        Ok(zel_core::protocol::Response {
                            data: bytes::Bytes::new()
                        })
                    })
                }
            }
        )
    }
}

/// Render registration code for a notification
pub fn render_notification_registration(
    notif: &NotificationDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let notif_name = &notif.rpc_name;
    let rust_method = &notif.signature.sig.ident;
    let param_idents: Vec<_> = notif.params.iter().map(|p| &p.ident).collect();

    // Generate typed receiver name
    let typed_receiver_name = generate_typed_name(trait_name, rust_method, "Receiver");

    let deserialize_params = generate_param_deserialization(&notif.params);

    quote! {
        service_builder.notification_resource(
            #notif_name,
            {
                let service = self.clone();
                move |
                    ctx: zel_core::protocol::RequestContext,
                    req: zel_core::protocol::Request,
                    inner_rx: tokio_util::codec::FramedRead<
                        iroh::endpoint::RecvStream,
                        tokio_util::codec::LengthDelimitedCodec
                    >,
                    inner_tx: tokio_util::codec::FramedWrite<
                        iroh::endpoint::SendStream,
                        tokio_util::codec::LengthDelimitedCodec
                    >
                | {
                    let service = service.clone();
                    Box::pin(async move {
                        // Extract body (for parameter support)
                        let data = match &req.body {
                            zel_core::protocol::Body::Notify(data) => data.as_ref(),
                            zel_core::protocol::Body::Rpc(data) => data.as_ref(),
                            _ => &[] as &[u8],
                        };

                        // Deserialize params (if any)
                        #deserialize_params

                        // Create NotificationSink and typed receiver
                        let sink = zel_core::protocol::NotificationSink::new(inner_tx);
                        let receiver = #typed_receiver_name { inner: inner_rx, sink };

                        // Call user's notification method with context and typed receiver
                        service.#rust_method(ctx, receiver, #(#param_idents),*).await?;

                        Ok(zel_core::protocol::Response {
                            data: bytes::Bytes::new()
                        })
                    })
                }
            }
        )
    }
}

/// Render registration code for a stream
pub fn render_stream_registration(stream: &StreamDescription) -> TokenStream2 {
    let stream_name = &stream.rpc_name;
    let rust_method = &stream.signature.sig.ident;
    let param_idents: Vec<_> = stream.params.iter().map(|p| &p.ident).collect();

    let deserialize_params = generate_param_deserialization(&stream.params);

    quote! {
        service_builder.stream_resource(
            #stream_name,
            {
                let service = self.clone();
                move |
                    ctx: zel_core::protocol::RequestContext,
                    req: zel_core::protocol::Request,
                    send: iroh::endpoint::SendStream,
                    recv: iroh::endpoint::RecvStream,
                | {
                    let service = service.clone();
                    Box::pin(async move {
                        // Extract body for parameter deserialization
                        let data = match &req.body {
                            zel_core::protocol::Body::Stream(data) => data.as_ref(),
                            _ => &[] as &[u8],
                        };

                        // Deserialize params (if any)
                        #deserialize_params

                        // Call user's stream method with context, raw streams, and params
                        service.#rust_method(ctx, send, recv, #(#param_idents),*).await?;

                        Ok(zel_core::protocol::Response {
                            data: bytes::Bytes::new()
                        })
                    })
                }
            }
        )
    }
}
