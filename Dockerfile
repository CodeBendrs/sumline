FROM rust:1.97-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release --bin sumline

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates poppler-utils && rm -rf /var/lib/apt/lists/* && useradd --create-home --uid 10001 invoice
WORKDIR /app
COPY --from=build /app/target/release/sumline /app/sumline
COPY deploy/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh && mkdir /app/data && chown invoice:invoice /app/data
# The entrypoint starts as root only to make a mounted volume writable, then runs the app as
# the unprivileged `invoice` user. The platform's PORT variable (Railway sets it) chooses the
# listening port; 3030 is the default elsewhere.
ENV INVOICE_DATA_DIR=/app/data INVOICE_DEMO=0 PORT=3030
EXPOSE 3030
HEALTHCHECK --interval=30s --timeout=8s --start-period=15s --retries=3 CMD ["/app/sumline", "--healthcheck"]
ENTRYPOINT ["/app/entrypoint.sh"]
