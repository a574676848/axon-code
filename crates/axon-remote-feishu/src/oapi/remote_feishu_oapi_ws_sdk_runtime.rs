use super::*;

impl RemoteFeishuOapiWsSdkRuntime {
    /// 连接 Feishu WS，并按退出条件读取 binary frame；bootstrap、连接或 frame 处理失败时返回错误。
    pub async fn run_until_closed_with_control<F, Fut>(
        config: RemoteFeishuOapiWsSdkConfig,
        control: RemoteFeishuOapiWsLoopControl,
        mut on_payload: F,
    ) -> AxonResult<RemoteFeishuOapiWsLoopReport>
    where
        F: FnMut(RemoteFeishuOapiWsIncomingPayload) -> Fut,
        Fut: Future<Output = AxonResult<Option<Value>>>,
    {
        let endpoint = Self::bootstrap_endpoint(&config).await?;
        let service_id = Self::service_id_from_endpoint(&endpoint.url)?;
        let mut client_config = endpoint.client_config.unwrap_or_default();
        control.replace_client_config(client_config);
        axon_logger::debug(&format!(
            "Feishu WS 开始连接: service_id={service_id}, host={}",
            Url::parse(&endpoint.url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        ));
        ensure_rustls_crypto_provider();
        let (mut socket, _) = timeout(
            Duration::from_secs(WS_CONNECT_TIMEOUT_SECS),
            connect_async_with_config(
                &endpoint.url,
                Some(feishu_oapi_ws_transport_config()),
                false,
            ),
        )
        .await
        .map_err(|_| {
            AxonError::provider_transport(
                format!("Feishu OAPI WS 连接超时（>{WS_CONNECT_TIMEOUT_SECS}s）"),
                None,
            )
        })?
        .map_err(|error| {
            axon_logger::error(&format!("Feishu WS 连接失败: {error}"));
            AxonError::provider_transport(format!("Feishu OAPI WS 连接失败: {error}"), None)
        })?;
        axon_logger::debug(&format!("Feishu WS 连接成功: service_id={service_id}"));
        control.signal_connected();
        let mut loop_report = RemoteFeishuOapiWsLoopReport {
            connected: true,
            received_binary_frames: 0,
            received_data_frames: 0,
            handled_events: 0,
            skipped_data_frames: 0,
            sent_pings: 0,
            received_pongs: 0,
            requested_stops: 0,
        };
        let mut ping_interval = feishu_oapi_ws_ping_interval(client_config);
        ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let max_wait = control.max_duration.map(sleep);
        tokio::pin!(max_wait);
        let mut cancellation_poll = tokio::time::interval(Duration::from_millis(100));
        cancellation_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let read_idle_timeout = Duration::from_secs(WS_READ_IDLE_TIMEOUT_SECS);
        let mut fragments = FragmentBuffer::new();
        let write_lock = Arc::new(Mutex::new(()));
        send_ws_message(
            &mut socket,
            &write_lock,
            Message::Binary(RemoteFeishuOapiWsSdkRuntime::encode_ping_frame(service_id)?.into()),
        )
        .await?;
        loop_report.sent_pings += 1;
        loop {
            tokio::select! {
                _ = cancellation_poll.tick(), if control.cancellation.is_some() => {
                    if control
                        .cancellation
                        .as_ref()
                        .is_some_and(|cancelled| {
                            cancelled.load(std::sync::atomic::Ordering::SeqCst)
                        })
                    {
                        break;
                    }
                }
                () = async {
                    match max_wait.as_mut().as_pin_mut() {
                        Some(duration) => duration.await,
                        None => std::future::pending::<()>().await,
                    }
                } => break,
                _ = ping_interval.tick() => {
                    send_ws_message(
                        &mut socket,
                        &write_lock,
                        Message::Binary(RemoteFeishuOapiWsSdkRuntime::encode_ping_frame(service_id)?.into()),
                    ).await?;
                    loop_report.sent_pings += 1;
                    axon_logger::debug(&format!(
                        "Feishu WS ping 已发送: service_id={service_id}, count={}",
                        loop_report.sent_pings
                    ));
                }
                message = timeout(read_idle_timeout, socket.next()) => {
                    let message = message.map_err(|_| {
                        axon_logger::error(&format!(
                            "Feishu WS 读取空闲超时: timeout_secs={WS_READ_IDLE_TIMEOUT_SECS}, service_id={service_id}"
                        ));
                        AxonError::provider_transport(
                            format!(
                                "Feishu OAPI WS 读取空闲超时（>{WS_READ_IDLE_TIMEOUT_SECS}s）"
                            ),
                            None,
                        )
                    })?;
                    let Some(message) = message else {
                        axon_logger::warning(&format!(
                            "Feishu WS 对端 EOF: service_id={service_id}"
                        ));
                        return Err(AxonError::provider_transport(
                            "Feishu OAPI WS 对端已关闭连接",
                            None,
                        ));
                    };
                    let message = message.map_err(|error| {
                        axon_logger::error(&format!(
                            "Feishu WS 读取帧失败: service_id={service_id}, error={error}"
                        ));
                        AxonError::provider_transport(
                            format!("Feishu OAPI WS 读取失败: {error}"),
                            None,
                        )
                    })?;
                    let bytes = match message {
                        Message::Binary(bytes) => {
                            loop_report.received_binary_frames += 1;
                            bytes
                        }
                        Message::Ping(payload) => {
                            axon_logger::debug(&format!(
                                "Feishu WS 收到协议 Ping，发送 Pong: service_id={service_id}"
                            ));
                            send_ws_message(&mut socket, &write_lock, Message::Pong(payload)).await.map_err(|error| {
                                axon_logger::error(&format!(
                                    "Feishu WS Pong 发送失败: service_id={service_id}, error={error:?}"
                                ));
                                AxonError::provider_transport(
                                    format!("Feishu OAPI WS Pong 发送失败: {error:?}"),
                                    None,
                                )
                            })?;
                            continue;
                        }
                        Message::Pong(_) => {
                            axon_logger::debug(&format!(
                                "Feishu WS 收到协议 Pong: service_id={service_id}"
                            ));
                            continue;
                        }
                        Message::Close(frame) => {
                            let reason = close_frame_reason(frame.as_ref());
                            axon_logger::warning(&format!(
                                "Feishu WS 收到 Close: service_id={service_id}, {reason}"
                            ));
                            let _ = send_ws_message(&mut socket, &write_lock, Message::Close(frame)).await;
                            return Err(AxonError::provider_transport(
                                format!("Feishu OAPI WS 对端关闭连接: {reason}"),
                                None,
                            ));
                        }
                        Message::Text(_) | Message::Frame(_) => continue,
                    };
                    let frame = decode_frame(&bytes)?;
                    if frame.method == FRAME_TYPE_DATA {
                        loop_report.received_data_frames += 1;
                    }
                    let Some(frame) = combine_fragmented_frame(frame, &mut fragments)? else {
                        continue;
                    };
                    let report = AssertUnwindSafe(Self::handle_decoded_frame(frame, &mut on_payload))
                        .catch_unwind()
                        .await
                        .map_err(|_| {
                            axon_logger::error("Feishu WS 消息处理 panic，已隔离当前消息");
                            AxonError::provider_transport("Feishu OAPI WS 消息处理异常", None)
                        })??;
                    let should_stop_after_response = report.stop_requested;
                    let skipped_frame = report.incoming.is_none()
                        && report.response_frame.is_none()
                        && report.client_config.is_none();
                    if let Some(config) = report.client_config {
                        client_config = config;
                        control.replace_client_config(client_config);
                        ping_interval = feishu_oapi_ws_ping_interval(client_config);
                        ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                        loop_report.received_pongs += 1;
                    }
                    if let Some(response) = report.response_frame {
                        send_ws_message(
                            &mut socket,
                            &write_lock,
                            Message::Binary(response.into()),
                        )
                        .await
                        .map_err(|error| {
                            axon_logger::error(&format!("Feishu WS 响应失败: {error:?}"));
                            AxonError::provider_transport(
                                format!("Feishu OAPI WS 响应失败: {error:?}"),
                                None,
                            )
                        })?;
                    }
                    if report.incoming.is_some() {
                        loop_report.handled_events += 1;
                    } else if skipped_frame {
                        loop_report.skipped_data_frames += 1;
                    }
                    if should_stop_after_response {
                        loop_report.requested_stops += 1;
                        break;
                    }
                    if control
                        .max_events
                        .is_some_and(|max_events| loop_report.handled_events >= max_events)
                    {
                        break;
                    }
                }
            }
        }
        Ok(loop_report)
    }
}
