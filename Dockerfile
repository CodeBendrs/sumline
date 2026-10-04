FROM rust:1.97-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release --bin invoice-tracker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates poppler-utils && rm -rf /var/lib/apt/lists/* && useradd --create-home --uid 10001 invoice
WORKDIR /app
COPY --from=build /app/target/release/invoice-tracker /app/invoice-tracker
RUN mkdir /app/data && chown invoice:invoice /app/data
USER invoice
ENV INVOICE_BIND=0.0.0.0:3030 INVOICE_DATA_DIR=/app/data INVOICE_DEMO=0
EXPOSE 3030
ENTRYPOINT ["/app/invoice-tracker"]
