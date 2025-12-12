# Changelog

## [0.5.0] - 2025-12-12

### Changed

- **BREAKING**: Replaced `into_service_builder()` with `register_service()` in the `#[zel_service]` macro
  - The new `register_service()` method automatically calls `.service(name)` using the service name from the macro attribute
  - Migration: Replace all existing `.service("name").into_service_builder(builder)` with `.register_service(builder)`. Yay code magic!

## [0.1.0..0.4.0] - Previous Releases

Initial development establishing core framework:

- Type-safe RPC with `#[zel_service]` macro
- Four endpoint types: methods, subscriptions, notifications, raw streams
- Three-tier extension system (server/connection/request)
- Circuit breakers and automatic retries for P2P resilience
- Iroh integration (holepunching, relays, network change detection) <3 Iroh
