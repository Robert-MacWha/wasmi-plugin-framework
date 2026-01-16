[PLUGIN_0] [50.278] [test_plugin]  INFO test_plugin: Starting plugin...
[WORKER_0] [50.279] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[PLUGIN_0] [50.279] [test_plugin]  INFO wasmi_plugin_pdk::server: PluginServer: Received request: RpcRequest { jsonrpc: "2.0", id: 1, method: "call_many", params: Number(200) }

#? These are logs from a background read loop running in the plugin to enable async calls. But they're not relevant right now
[PLUGIN_0] [50.279] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver_Background: Awaiting incoming message...
[WORKER_0] [50.279] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[WORKER_0] [50.279] INFO /runtime/shared_pipe.rs:168  SharedPipe_WORKER_STDIN::read WouldBlock
[PLUGIN_0] [50.279] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver_Background: Message processed, yielding...

[PLUGIN_0] [50.279] [test_plugin]  INFO test_plugin: Making call 0
[PLUGIN_0] [50.280] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Sending request ID 0
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:187  SharedPipe_WORKER_STDOUT::write called, buf len=55
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:215  SharedPipe_WORKER_STDOUT::write wrote 55, new_w=55
[PLUGIN_0] [50.280] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Locking reader...
[PLUGIN_0] [50.280] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Waiting for response id=0
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:168  SharedPipe_WORKER_STDIN::read WouldBlock
[PLUGIN_0] [50.280] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: WouldBlock, yielding... id=0
[WORKER_0] [50.280] INFO wasi/wasi_ctx.rs:569  wasi poll_oneoff(1046552, 1046600, 1, 1046636)
[WORKER_0] [50.280] INFO wasi/wasi_ctx.rs:670  poll_oneoff: waiting for stdin or clock timeout...
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:241  SharedPipe_WORKER_STDIN::wait_ready called, timeout=inf
[WORKER_0] [50.280] INFO /runtime/shared_pipe.rs:250  SharedPipe_WORKER_STDIN::wait_ready checking r=59, current_w=59
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:277  SharedPipe_WORKER_STDIN::wait_ready resolved
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Resuming... id=0
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Received message: RpcResponse(RpcResponse { jsonrpc: "2.0", id: 0, result: String("pong") })
[PLUGIN_0] [50.281] [test_plugin]  INFO test_plugin: Call 0 got response: String("pong")

[PLUGIN_0] [50.281] [test_plugin]  INFO test_plugin: Making call 1
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Sending request ID 1
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:187  SharedPipe_WORKER_STDOUT::write called, buf len=55
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:215  SharedPipe_WORKER_STDOUT::write wrote 55, new_w=110
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Locking reader...
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Waiting for response id=1
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:168  SharedPipe_WORKER_STDIN::read WouldBlock
[PLUGIN_0] [50.281] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: WouldBlock, yielding... id=1
[WORKER_0] [50.281] INFO wasi/wasi_ctx.rs:569  wasi poll_oneoff(1046552, 1046600, 1, 1046636)
[WORKER_0] [50.281] INFO wasi/wasi_ctx.rs:670  poll_oneoff: waiting for stdin or clock timeout...
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:241  SharedPipe_WORKER_STDIN::wait_ready called, timeout=inf
[WORKER_0] [50.281] INFO /runtime/shared_pipe.rs:250  SharedPipe_WORKER_STDIN::wait_ready checking r=100, current_w=100
[WORKER_0] [50.282] INFO /runtime/shared_pipe.rs:277  SharedPipe_WORKER_STDIN::wait_ready resolved
[PLUGIN_0] [50.282] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Resuming... id=1
[WORKER_0] [50.282] INFO /runtime/shared_pipe.rs:152  SharedPipe_WORKER_STDIN::read called, buf len=8192
[PLUGIN_0] [50.282] [test_plugin]  INFO wasmi_plugin_pdk::transport_driver: TransportDriver: Received message: RpcResponse(RpcResponse { jsonrpc: "2.0", id: 1, result: String("pong") })
[PLUGIN_0] [50.282] [test_plugin]  INFO test_plugin: Call 1 got response: String("pong")