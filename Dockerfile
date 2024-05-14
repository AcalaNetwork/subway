FROM rust:buster as builder
WORKDIR /app

COPY . .

RUN cargo build --release --verbose

# =============

FROM phusion/baseimage:focal-1.2.0
LABEL maintainer="hello@acala.network"

RUN useradd -m -u 1000 -U -s /bin/sh -d /app docker

WORKDIR /app

RUN apt-get update && apt-get install tini

COPY --from=builder /app/target/release/subway /usr/local/bin

# checks
RUN ldd /usr/local/bin/subway && \
	/usr/local/bin/subway --version

# Shrinking
RUN rm -rf /usr/lib/python* && \
	rm -rf /usr/sbin /usr/share/man

USER docker
EXPOSE 9944

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/subway"]
