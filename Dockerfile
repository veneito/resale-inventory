FROM rust:1.98-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/resale-inventory /resale-inventory
ENV RI_DB=/data/inventory.db RI_ADDR=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/resale-inventory"]
