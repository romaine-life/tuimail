# tuimail's session on the home cluster (`tuimail session`, TUI-17): the house's one sign-in to
# each mail account, which the PCs' tuimail reach mail through. Built here for Linux, where the
# session's store is SQLCipher with its own OpenSSL (Cargo.toml). pipeline-templates'
# cluster-service suite builds it, runs `--self-test` in it read-only, and pins it in
# k8s/deployment.yaml.
FROM artifacts.home.romaine.life/dockerhub/library/rust:1.99-trixie@sha256:8a4e90c7946b74626488b1446f712c0fe0ed1a596cbd3d871684bc9c5433ae07 AS build
# Crates through Artifact Keeper's cache, as every download at home goes.
RUN printf '[source.crates-io]\nreplace-with = "home"\n\n[source.home]\nregistry = "sparse+https://artifacts.home.romaine.life/cargo/crates/"\n' \
    > "$CARGO_HOME/config.toml"
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY themes ./themes
# The release's version and commit, which `tuimail --version` and the log say.
ARG APP_VERSION
ARG SOURCE_SHA
RUN VERSION="$APP_VERSION" SOURCE_SHA="$SOURCE_SHA" BUILD_TIME="$(date +%s)" \
    cargo build --release --locked --bin tuimail

FROM artifacts.home.romaine.life/dockerhub/library/debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
# apt reaches the cache over https before ca-certificates is installed, so its first two runs
# name this bundle, which apt doesn't find on its own (as spotify-player's image does).
COPY --from=artifacts.home.romaine.life/dockerhub/library/alpine:3.24.2@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# ca-certificates: the system's roots, which TLS to the mail servers, Google and OpenBao is
# checked against.
RUN sed -i 's#https\?://deb.debian.org/#https://artifacts.home.romaine.life/debian/#' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt update \
    && apt-get -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/tuimail /usr/local/bin/tuimail
USER 65534:65534
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/tuimail", "session"]
