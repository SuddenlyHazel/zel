use crate::service::{MethodDescription, ServiceDescription, SubscriptionDescription};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

pub fn render_service(service: &ServiceDescription) -> TokenStream2 {
    let server_trait = render_server_trait(service);
    let client_struct = render_client_struct(service);

    quote! {
        #server_trait
        #client_struct
    }
}

fn render_server_trait(service: &ServiceDescription) -> TokenStream2 {
    let trait_name = &service.trait_ident;
    let server_name = quote::format_ident!("{}Server", trait_name);

    // Render method signatures
    let methods: Vec<_> = service
        .methods
        .iter()
        .map(|m| {
            let sig = &m.signature.sig;
            let attrs = &m.signature.attrs;

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Render subscription signatures with injected sink parameter
    let subscriptions: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| {
            let mut sig = s.signature.sig.clone();
            let attrs = &s.signature.attrs;

            // Insert sink parameter as second argument (after &self)
            let sink_param: syn::FnArg = syn::parse_quote! {
                sink: zel_core::protocol::SubscriptionSink
            };
            sig.inputs.insert(1, sink_param);

            quote! {
                #(#attrs)*
                #sig;
            }
        })
        .collect();

    // Generate the into_service_builder method body
    let into_builder_body = render_into_builder_body(service);

    quote! {
        #[async_trait::async_trait]
        pub trait #server_name: Clone + Send + Sync + 'static {
            #(#methods)*
            #(#subscriptions)*

            /// Convert this service implementation into a ServiceBuilder with registered resources
            fn into_service_builder(
                self,
                service_builder: zel_core::protocol::ServiceBuilder<'static>
            ) -> zel_core::protocol::ServiceBuilder<'static> {
                #into_builder_body
            }
        }
    }
}

fn render_into_builder_body(service: &ServiceDescription) -> TokenStream2 {
    let method_registrations: Vec<_> = service
        .methods
        .iter()
        .map(render_method_registration)
        .collect();

    let subscription_registrations: Vec<_> = service
        .subscriptions
        .iter()
        .map(render_subscription_registration)
        .collect();

    quote! {
        let service_builder = service_builder;

        #(
            let service_builder = #method_registrations;
        )*

        #(
            let service_builder = #subscription_registrations;
        )*

        service_builder
    }
}

fn render_method_registration(method: &MethodDescription) -> TokenStream2 {
    let method_name = &method.rpc_name;
    let rust_method = &method.signature.sig.ident;
    let param_idents: Vec<_> = method.params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = method.params.iter().map(|p| &p.ty).collect();

    let deserialize_params = if param_idents.is_empty() {
        quote! {
            // No parameters
        }
    } else if param_idents.len() == 1 {
        let ident = &param_idents[0];
        let ty = &param_types[0];
        quote! {
            let #ident: #ty = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameter: {}", e)
                ))?;
        }
    } else {
        quote! {
            let params: (#(#param_types),*) = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameters: {}", e)
                ))?;
            let (#(#param_idents),*) = params;
        }
    };

    quote! {
        service_builder.rpc_resource(
            #method_name,
            {
                let service = self.clone();
                move |_conn: iroh::endpoint::Connection, req: zel_core::protocol::Request| {
                    let service = service.clone();
                    Box::pin(async move {
                        // Extract body
                        let zel_core::protocol::Body::Rpc(data) = &req.body else {
                            return Err(zel_core::protocol::ResourceError::CallbackError(
                                "Expected RPC body".into()
                            ));
                        };

                        // Deserialize params
                        #deserialize_params

                        // Call user's method
                        let result = service.#rust_method(#(#param_idents),*).await
                            .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                                e.to_string()
                            ))?;

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

fn render_subscription_registration(sub: &SubscriptionDescription) -> TokenStream2 {
    let sub_name = &sub.rpc_name;
    let rust_method = &sub.signature.sig.ident;
    let param_idents: Vec<_> = sub.params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = sub.params.iter().map(|p| &p.ty).collect();

    let deserialize_params = if param_idents.is_empty() {
        quote! {
            // No parameters
        }
    } else if param_idents.len() == 1 {
        let ident = &param_idents[0];
        let ty = &param_types[0];
        quote! {
            let #ident: #ty = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameter: {}", e)
                ))?;
        }
    } else {
        quote! {
            let params: (#(#param_types),*) = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameters: {}", e)
                ))?;
            let (#(#param_idents),*) = params;
        }
    };

    quote! {
        service_builder.subscription_resource(
            #sub_name,
            {
                let service = self.clone();
                move |
                    _conn: iroh::endpoint::Connection,
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
                        };

                        // Deserialize params (if any)
                        #deserialize_params

                        // Create SubscriptionSink wrapper
                        let sink = zel_core::protocol::SubscriptionSink::new(inner_sink);

                        // Call user's subscription method
                        service.#rust_method(sink, #(#param_idents),*).await
                            .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                                e.to_string()
                            ))?;

                        Ok(zel_core::protocol::Response {
                            data: bytes::Bytes::new()
                        })
                    })
                }
            }
        )
    }
}

fn render_client_struct(service: &ServiceDescription) -> TokenStream2 {
    let trait_name = &service.trait_ident;
    let client_name = quote::format_ident!("{}Client", trait_name);
    let service_name = &service.service_name;

    let client_methods: Vec<_> = service
        .methods
        .iter()
        .map(|m| render_client_method(m, service_name))
        .collect();

    let client_subscriptions: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| render_client_subscription(s, service_name, trait_name))
        .collect();

    let subscription_streams: Vec<_> = service
        .subscriptions
        .iter()
        .map(|s| render_subscription_stream(s, trait_name))
        .collect();

    quote! {
        #[derive(Clone)]
        pub struct #client_name {
            client: zel_core::protocol::RpcClient,
            service_name: String,
        }

        impl #client_name {
            pub fn new(client: zel_core::protocol::RpcClient) -> Self {
                Self {
                    client,
                    service_name: #service_name.to_string(),
                }
            }

            #(#client_methods)*
            #(#client_subscriptions)*
        }

        #(#subscription_streams)*
    }
}

fn render_client_method(method: &MethodDescription, _service_name: &str) -> TokenStream2 {
    let method_name = &method.signature.sig.ident;
    let rpc_name = &method.rpc_name;
    let params = &method.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let return_type = method
        .return_type
        .as_ref()
        .map(|ty| quote! { #ty })
        .unwrap_or_else(|| quote! { () });

    let serialize_params = if params.is_empty() {
        quote! {
            let body = bytes::Bytes::new();
        }
    } else if params.len() == 1 {
        let param = &param_idents[0];
        quote! {
            let body = serde_json::to_vec(&#param)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = bytes::Bytes::from(body);
        }
    } else {
        quote! {
            let params = (#(#param_idents),*);
            let body = serde_json::to_vec(&params)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = bytes::Bytes::from(body);
        }
    };

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<#return_type, zel_core::protocol::ClientError>
        {
            #serialize_params

            let response = self.client.call(&self.service_name, #rpc_name, body).await?;
            let result: #return_type = serde_json::from_slice(&response.data)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            Ok(result)
        }
    }
}

fn render_client_subscription(
    sub: &SubscriptionDescription,
    _service_name: &str,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &sub.signature.sig.ident;
    let rpc_name = &sub.rpc_name;
    let stream_name = quote::format_ident!(
        "{}{}",
        trait_name,
        capitalize_first(&method_name.to_string())
    );
    let stream_name = quote::format_ident!("{}Stream", stream_name);

    let params = &sub.params;
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    let serialize_params = if params.is_empty() {
        quote! {
            let body = None;
        }
    } else if params.len() == 1 {
        let param = &param_idents[0];
        quote! {
            let param_body = serde_json::to_vec(&#param)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = Some(bytes::Bytes::from(param_body));
        }
    } else {
        quote! {
            let params = (#(#param_idents),*);
            let param_body = serde_json::to_vec(&params)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = Some(bytes::Bytes::from(param_body));
        }
    };

    quote! {
        pub async fn #method_name(&self, #(#param_idents: #param_types),*)
            -> Result<#stream_name, zel_core::protocol::ClientError>
        {
            #serialize_params

            let stream = self.client.subscribe(&self.service_name, #rpc_name, body).await?;
            Ok(#stream_name { inner: stream })
        }
    }
}

fn render_subscription_stream(
    sub: &SubscriptionDescription,
    trait_name: &syn::Ident,
) -> TokenStream2 {
    let method_name = &sub.signature.sig.ident;
    let stream_name = quote::format_ident!(
        "{}{}",
        trait_name,
        capitalize_first(&method_name.to_string())
    );
    let stream_name = quote::format_ident!("{}Stream", stream_name);

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

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
