# syntax=docker/dockerfile:1
# Builds fully static (musl) binaries, then copies each into a distroless
# "static" image: no shell, no libc, no package manager, just CA certificates,
# a nonroot user and the binary.
#
#   docker build --target agent -t rust-agents/agent .
#   docker build --target mcp-server -t rust-agents/mcp-server .
# or simply: docker compose up --build

FROM rust:1.99-alpine AS build
# aws-lc-sys (rustls' crypto) compiles C code: it needs a C toolchain and cmake.
RUN apk add --no-cache musl-dev gcc g++ make cmake perl
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p agent -p mcp-server \
 && mkdir /out && cp target/release/agent target/release/mcp-server /out/

FROM gcr.io/distroless/static-debian12:nonroot AS mcp-server
COPY --from=build /out/mcp-server /mcp-server
ENV MCP_ADDR=0.0.0.0:8081
EXPOSE 8081
ENTRYPOINT ["/mcp-server"]

FROM gcr.io/distroless/static-debian12:nonroot AS agent
COPY --from=build /out/agent /agent
COPY knowledge-base /knowledge-base
ENV AGENT_ADDR=0.0.0.0:3000 KB_DIR=/knowledge-base
EXPOSE 3000
ENTRYPOINT ["/agent"]
