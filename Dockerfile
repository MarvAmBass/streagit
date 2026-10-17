# streagit – static musl binary in a minimal alpine image.
#
#   docker build --platform linux/amd64 -t streagit .
#   docker run --rm -p 127.0.0.1:8081:8081 -v /path/to/repos:/data:ro streagit
#   (default command: --listen 0.0.0.0:8081 --root /data --prefix /browse)
#
# Inside the container streagit listens on 0.0.0.0 so that the published port
# reaches it; `-p 127.0.0.1:8081:8081` keeps it reachable from the host only.
# streagit has no authentication – see "Security" in README.md.
#
# The build stage targets the native architecture of the builder (x86_64 or
# aarch64); `--platform` selects it. rust:alpine links musl statically.
FROM rust:alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked \
 && strip target/release/streagit \
 && ldd target/release/streagit 2>&1 | grep -qi "not a dynamic\|statically" || true

FROM alpine:3
RUN apk add --no-cache tini
COPY --from=build /src/target/release/streagit /usr/local/bin/streagit
COPY LICENSE LICENSE-stagit THIRD-PARTY-LICENSES.md /usr/share/licenses/streagit/
USER 1000:1000
EXPOSE 8081
ENTRYPOINT ["/sbin/tini", "--", "streagit"]
CMD ["--listen", "0.0.0.0:8081", "--root", "/data", "--prefix", "/browse"]
