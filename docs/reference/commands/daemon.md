# rvow daemon

Serve hook calls on a local socket until idle (started by the first hook)

## Usage

```sh
rvow daemon [OPTIONS]
```

## Options

| Option | Description |
| --- | --- |
| `--idle` `<IDLE>` | How long to wait without a call before exiting, such as 10m or 1h |
| `--host-pipe` | Serve the host pipe: write the questions on stdout, read the answers on stdin |
| `--ask-wait` `<ASK_WAIT>` | How long an ask waits for the host's answer before it reports waiting |
| `--print-hello` | Print the socket and the hello a client of this environment sends, as one JSON line, and exit |
