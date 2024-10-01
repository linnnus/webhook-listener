# Webhook listener

A webserver which runs commands when it receives webhook events from GitHub.

## Local development

To test the server in development, run:

```sh
$ rm -f /tmp/webhook-listener.sock
$ nix develop --command systemfd --socket unix::/tmp/webhook-listener.sock -- target/debug/webhook-listener
```

Then, in another terminal, run this command to send a sample event:

```sh
$ examples/sample_push_event.sh
```

`sample.http` contains a sample request signed with the key `mysecret`. The
payload from that request is found in `sample_push_payload.json` which is what
the above script sends.
