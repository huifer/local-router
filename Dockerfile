# ---- stage 1: React console ----
FROM node:24-alpine AS console
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ---- stage 2: Rust gateway ----
FROM rust:1.97-alpine AS build
WORKDIR /build
RUN apk add --no-cache musl-dev pkgconfig
# Dependencies first, so a source change does not rebuild the world.
COPY Cargo.toml Cargo.lock build.rs ./
# The dependency warm-up builds with a stub console (no npm in this image);
# the env is scoped to this RUN only, so the real build below sees no flag.
RUN mkdir src && echo "pub fn placeholder() {}" > src/lib.rs && \
    echo "fn main() {}" > src/main.rs && \
    LOCAL_ROUTER_SKIP_FRONTEND=1 cargo build --release && rm -rf src
COPY src/ ./src/
COPY tests/ ./tests/
COPY --from=console /build/dist ./frontend/dist
# The crate recompiles because src changed, and build.rs picks up the real
# dist: it exists, and frontend holds no newer source than it.
RUN cargo build --release --bin local-router

# ---- stage 3: runtime ----
FROM alpine:3.21
# /data must be writable by the runtime user even when a volume mounts over it.
RUN addgroup -S router -g 10001 && adduser -S router -u 10001 -G router \
 && mkdir -p /data && chown -R router:router /data
COPY --from=build /build/target/release/local-router /usr/local/bin/local-router
USER router
ENV LOCAL_ROUTER_HOME=/data \
    LOCAL_ROUTER_GATEWAY_PORT=3435 \
    LOCAL_ROUTER_GATEWAY_ADDR=0.0.0.0
VOLUME /data
# One port for the model API and the console.
EXPOSE 3435
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
  CMD wget -qO- http://127.0.0.1:3435/health || exit 1
ENTRYPOINT ["local-router"]
CMD ["serve"]
