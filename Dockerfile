FROM rust:1.97.1-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY openapi.yaml ./openapi.yaml
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl poppler-utils \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 ocr \
    && useradd --system --uid 10001 --gid ocr --no-create-home ocr

COPY --from=builder /build/target/release/ocr-service /usr/local/bin/ocr-service

USER 10001:10001
EXPOSE 8100
ENTRYPOINT ["/usr/local/bin/ocr-service"]
CMD ["serve", "--port", "8100"]
