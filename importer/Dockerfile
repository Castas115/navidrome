# Build in a full toolchain, ship a ~15MB image with nothing else in it.
FROM rust:1.92-alpine AS build

RUN apk add --no-cache musl-dev

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

FROM alpine:3.21

RUN adduser -D -u 1000 importer

COPY --from=build /src/target/release/navidrome-import /usr/local/bin/navidrome-import

USER importer
ENTRYPOINT ["/usr/local/bin/navidrome-import"]
