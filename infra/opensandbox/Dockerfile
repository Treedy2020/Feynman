ARG BASE_IMAGE=python:3.12-slim
FROM ${BASE_IMAGE}

RUN if command -v git >/dev/null && test -s /etc/ssl/certs/ca-certificates.crt; then exit 0; fi \
    && sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o APT::Sandbox::User=root -o Acquire::ForceIPv4=true -o Acquire::Retries=1 -o Acquire::https::Timeout=20 update \
    && apt-get -o APT::Sandbox::User=root -o Acquire::ForceIPv4=true -o Acquire::Retries=1 -o Acquire::https::Timeout=20 install -y --no-install-recommends git ca-certificates \
    && rm -r /var/lib/apt/lists/*

# Build context contains the vendor files from a pinned Linux Codex npm package.
# No model auth, DWS credentials, Docker socket, or host filesystem is included.
COPY codex/ /opt/feynman/vendor/
RUN mkdir -p /opt/feynman/bin && ln -s /opt/feynman/vendor/bin/codex /opt/feynman/bin/codex
ENV PATH="/opt/feynman/bin:/opt/feynman/vendor/codex-path:${PATH}"
WORKDIR /workspace
