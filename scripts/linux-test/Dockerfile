# Linux for sesh-mux's tests that only build there, run from a Mac by scripts/mux-linux-test.sh.
FROM rust:1-bookworm
ARG ZIG=0.16.0
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends python3 xz-utils git zsh procps dbus && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL "https://ziglang.org/download/$ZIG/zig-$(uname -m)-linux-$ZIG.tar.xz" | tar xJ -C /opt \
    && ln -s "/opt/zig-$(uname -m)-linux-$ZIG/zig" /usr/local/bin/zig \
    && useradd -m tester && mkdir /target && chown tester /target
USER tester
ENV CARGO_HOME=/home/tester/.cargo CARGO_TARGET_DIR=/target
