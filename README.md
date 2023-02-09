# Subway

Substrate JSON RPC Gateway.

This is a generalized JSON RPC proxy server with features specifically designed for Substrate RPC and Ethereum RPC.

## Getting Started

Quick start: `cargo run -- --config config.yml`

This will run a proxy server with [config.yml](config.yml) as the configuration file.

## Environment Variables

- `RUST_LOG`
  - Log level. Default: `info`.
- `PORT`
  - Override port configuration in config file.
- `LOG_FORMAT`
  - Log format. Default: `full`.
  - Options: `full`, `pretty`, `json`, `compact`

## Features

Subway is build with middleware pattern.

### Middlewares

**Method Middlewares**

- Cache
  - Cache responses from upstream middleware.
- Call
  - Forward requests to upstream servers.
- Inject Params (Substrate)
  - For Substrate RPC
  - Inject optional `blockAt` or `blockHash` params to requests to ensure downstream middleware such as cache can work properly.
- Inject Params (Ethereum)
  - For Ethereum RPC
  - Inject optional `defaultBlock` parameter to requests to ensure downstream middleware such as cache can work properly.
- Subscription
  - Forward requests to upstream servers.
  - TODO: Merge duplicated subscriptions.
- TODO: Rate Limit
  - Rate limit requests from downstream middleware.
- TODO: Parameter filter
  - Deny requests with invalid parameters.

### Additional features

- Advance JSON RPC Client
  - Supports multiple upstream servers and rotate & reconnect on failure.
  - TODO: Load balance requests to upstream servers.
- Batch Request
  - TODO: Process requests individually so they can be cached properly by downstream middlewares.
  - TODO: Limit batch size, request size and response size.
- TODO: Metrics
  - Getting insights of the RPC calls and server performance.