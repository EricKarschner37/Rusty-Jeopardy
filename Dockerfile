FROM rust:1.81-alpine as builder
RUN apk add openssl-dev musl-dev openssl-libs-static
WORKDIR /usr/src/jeopardy
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY src ./src
RUN rustup target add x86_64-unknown-linux-musl
RUN cargo install --path . --target x86_64-unknown-linux-musl

FROM alpine:latest
RUN apk add --update --no-cache python3 py3-pip py3-virtualenv
COPY --from=builder /usr/local/cargo/bin/jeopardy /usr/local/bin/jeopardy

ENV JEOPARDY_GAME_ROOT=/games/
ENV J_GAME_ROOT=/games

ENV PYTHONUNBUFFERED=1
COPY requirements.txt .
RUN python3 -m venv /opt/venv && source /opt/venv/bin/activate
ENV PATH="/opt/venv/bin:$PATH"
RUN pip3 install -Ur requirements.txt

EXPOSE 10001

CMD ["jeopardy"]
