#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, anyhow, bail};
    use ashpd::desktop::remote_desktop::{
        Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
    };
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::screenshot::{AvailableTargets, Screenshot};
    use ashpd::desktop::{Session, remote_desktop::SelectedDevices, screencast::Stream};
    use atspi::State;
    use atspi::connection::AccessibilityConnection;
    use atspi::proxy::accessible::AccessibleProxy;
    use atspi::proxy::action::ActionProxy;
    use atspi::proxy::editable_text::EditableTextProxy;
    use atspi::proxy::proxy_ext::ProxyExt;
    use atspi::proxy::text::TextProxy;
    use base64::Engine as _;
    use futures::future::BoxFuture;
    use serde_json::{Value, json};
    use std::io::{self, BufRead, Write};

    const TOOLS: [&str; 10] = [
        "list_apps",
        "get_app_state",
        "click",
        "drag",
        "press_key",
        "type_text",
        "perform_secondary_action",
        "set_value",
        "select_text",
        "scroll",
    ];

    struct RemoteInput {
        portal: RemoteDesktop,
        session: Session<RemoteDesktop>,
        stream: Option<Stream>,
    }

    impl RemoteInput {
        async fn connect() -> Result<Self> {
            let portal = RemoteDesktop::new()
                .await
                .context("connect to the XDG RemoteDesktop portal")?;
            let screencast = Screencast::new()
                .await
                .context("connect to the XDG ScreenCast portal")?;
            let session = portal.create_session(Default::default()).await?;
            portal
                .select_devices(
                    &session,
                    SelectDevicesOptions::default()
                        .set_devices(DeviceType::Keyboard | DeviceType::Pointer),
                )
                .await?
                .response()?;
            let sources: enumflags2::BitFlags<SourceType> = SourceType::Monitor.into();
            screencast
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(CursorMode::Hidden)
                        .set_sources(sources)
                        .set_multiple(false),
                )
                .await?
                .response()?;
            let selected: SelectedDevices = portal
                .start(&session, None, Default::default())
                .await?
                .response()?;
            let stream = selected.streams().first().cloned();
            Ok(Self {
                portal,
                session,
                stream,
            })
        }

        fn stream_id(&self) -> Result<u32> {
            self.stream
                .as_ref()
                .map(Stream::pipe_wire_node_id)
                .ok_or_else(|| {
                    anyhow!(
                        "RemoteDesktop did not return a screen stream for absolute pointer input"
                    )
                })
        }

        async fn move_absolute(&self, x: f64, y: f64) -> Result<()> {
            self.portal
                .notify_pointer_motion_absolute(
                    &self.session,
                    self.stream_id()?,
                    x,
                    y,
                    Default::default(),
                )
                .await?;
            Ok(())
        }

        async fn click(&self, x: f64, y: f64, button: i32, count: u64) -> Result<()> {
            self.move_absolute(x, y).await?;
            for _ in 0..count {
                self.portal
                    .notify_pointer_button(
                        &self.session,
                        button,
                        KeyState::Pressed,
                        Default::default(),
                    )
                    .await?;
                self.portal
                    .notify_pointer_button(
                        &self.session,
                        button,
                        KeyState::Released,
                        Default::default(),
                    )
                    .await?;
            }
            Ok(())
        }

        async fn drag(&self, from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<()> {
            self.move_absolute(from_x, from_y).await?;
            self.portal
                .notify_pointer_button(&self.session, 0x110, KeyState::Pressed, Default::default())
                .await?;
            self.move_absolute(to_x, to_y).await?;
            self.portal
                .notify_pointer_button(&self.session, 0x110, KeyState::Released, Default::default())
                .await?;
            Ok(())
        }

        async fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
            if dx != 0 {
                self.portal
                    .notify_pointer_axis_discrete(
                        &self.session,
                        Axis::Horizontal,
                        dx,
                        Default::default(),
                    )
                    .await?;
            }
            if dy != 0 {
                self.portal
                    .notify_pointer_axis_discrete(
                        &self.session,
                        Axis::Vertical,
                        dy,
                        Default::default(),
                    )
                    .await?;
            }
            Ok(())
        }

        async fn key(&self, keysym: i32) -> Result<()> {
            self.key_state(keysym, KeyState::Pressed).await?;
            self.key_state(keysym, KeyState::Released).await?;
            Ok(())
        }

        async fn key_state(&self, keysym: i32, state: KeyState) -> Result<()> {
            self.portal
                .notify_keyboard_keysym(&self.session, keysym, state, Default::default())
                .await?;
            Ok(())
        }
    }

    async fn proxy<'a>(
        connection: &'a AccessibilityConnection,
        reference: &atspi::ObjectRefOwned,
    ) -> Result<AccessibleProxy<'a>> {
        let name = reference
            .name()
            .ok_or_else(|| anyhow!("AT-SPI returned a null object"))?;
        Ok(AccessibleProxy::builder(connection.connection())
            .destination(name.clone())?
            .path(reference.path().clone())?
            .build()
            .await?)
    }

    async fn app_roots(connection: &AccessibilityConnection) -> Result<Vec<atspi::ObjectRefOwned>> {
        Ok(connection
            .root_accessible_on_registry()
            .await?
            .get_children()
            .await?)
    }

    async fn app_by_name<'a>(
        connection: &'a AccessibilityConnection,
        wanted: &str,
    ) -> Result<AccessibleProxy<'a>> {
        let wanted = wanted.to_lowercase();
        for reference in app_roots(connection).await? {
            let app = proxy(connection, &reference).await?;
            let name = app.name().await.unwrap_or_default();
            if name.to_lowercase() == wanted || name.to_lowercase().contains(&wanted) {
                return Ok(app);
            }
        }
        bail!("application is not available through AT-SPI: {wanted}");
    }

    fn collect<'a>(
        connection: &'a AccessibilityConnection,
        node: &'a AccessibleProxy<'a>,
        out: &'a mut Vec<Value>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if out.len() >= 600 {
                return Ok(());
            }
            let index = out.len();
            let name = node.name().await.unwrap_or_default();
            let description = node.description().await.unwrap_or_default();
            let role = node
                .get_role_name()
                .await
                .unwrap_or_else(|_| "unknown".into());
            let state = node.get_state().await.unwrap_or_default();
            let mut item = json!({
                "index": index,
                "role": role,
                "name": name,
                "description": description,
                "enabled": state.contains(State::Enabled),
            });
            if let Ok(interfaces) = node.proxies().await {
                if let Ok(component) = interfaces.component().await {
                    if let Ok((x, y, width, height)) =
                        component.get_extents(atspi::CoordType::Screen).await
                    {
                        item["bounds"] = json!([x, y, width, height]);
                    }
                }
            }
            out.push(item);
            for child in node.get_children().await.unwrap_or_default() {
                if out.len() >= 600 {
                    break;
                }
                let child = proxy(connection, &child).await?;
                collect(connection, &child, out).await?;
            }
            Ok(())
        })
    }

    async fn capture() -> Result<Option<String>> {
        let response = Screenshot::request()
            .target(AvailableTargets::Screen)
            .interactive(false)
            .modal(false)
            .send()
            .await?
            .response()?;
        let uri = url::Url::parse(response.uri().as_str())?;
        let path = uri
            .to_file_path()
            .map_err(|_| anyhow!("Screenshot portal returned a non-file URI"))?;
        let bytes =
            std::fs::read(&path).with_context(|| format!("read screenshot {}", path.display()))?;
        let _ = std::fs::remove_file(&path);
        Ok(Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )))
    }

    async fn list_apps(connection: &AccessibilityConnection) -> Result<Vec<Value>> {
        let mut apps = Vec::new();
        for reference in app_roots(connection).await? {
            let app = proxy(connection, &reference).await?;
            let name = app.name().await.unwrap_or_default();
            if name.is_empty() || name == "gnome-shell" || name == "ibus-extension-gtk3" {
                continue;
            }
            let state = app.get_state().await.unwrap_or_default();
            if state.contains(State::Visible) {
                apps.push(json!({"id": name, "displayName": name, "isRunning": true}));
            }
        }
        Ok(apps)
    }

    async fn state(connection: &AccessibilityConnection, name: &str) -> Result<Value> {
        let app = app_by_name(connection, name).await?;
        let app_name = app.name().await.unwrap_or_else(|_| name.to_string());
        let mut elements = Vec::new();
        collect(connection, &app, &mut elements).await?;
        Ok(
            json!({"app": app_name, "screenshot": capture().await?.map(|url| json!({"url": url})), "text": serde_json::to_string(&elements)?}),
        )
    }

    fn keysym(token: &str) -> Option<i32> {
        let lower = token.to_ascii_lowercase();
        Some(match lower.as_str() {
            "return" | "enter" => 0xff0d,
            "tab" => 0xff09,
            "escape" | "esc" => 0xff1b,
            "backspace" => 0xff08,
            "delete" | "del" => 0xffff,
            "up" => 0xff52,
            "down" => 0xff54,
            "left" => 0xff51,
            "right" => 0xff53,
            "home" => 0xff50,
            "end" => 0xff57,
            "space" => 0x20,
            "shift" => 0xffe1,
            "ctrl" | "control" => 0xffe3,
            "alt" => 0xffe9,
            "super" | "meta" | "command" => 0xffeb,
            "f1" => 0xffbe,
            "f2" => 0xffbf,
            "f3" => 0xffc0,
            "f4" => 0xffc1,
            "f5" => 0xffc2,
            "f6" => 0xffc3,
            "f7" => 0xffc4,
            "f8" => 0xffc5,
            "f9" => 0xffc6,
            "f10" => 0xffc7,
            "f11" => 0xffc8,
            "f12" => 0xffc9,
            _ if token.chars().count() == 1 => token.chars().next()?.to_ascii_lowercase() as i32,
            _ => return None,
        })
    }

    fn character_keysym(character: char) -> (Option<i32>, i32) {
        let shifted = match character {
            '~' => Some('`'),
            '!' => Some('1'),
            '@' => Some('2'),
            '#' => Some('3'),
            '$' => Some('4'),
            '%' => Some('5'),
            '^' => Some('6'),
            '&' => Some('7'),
            '*' => Some('8'),
            '(' => Some('9'),
            ')' => Some('0'),
            '_' => Some('-'),
            '+' => Some('='),
            '{' => Some('['),
            '}' => Some(']'),
            '|' => Some('\\'),
            ':' => Some(';'),
            '"' => Some('\''),
            '<' => Some(','),
            '>' => Some('.'),
            '?' => Some('/'),
            _ => None,
        };
        if let Some(base) = shifted {
            return (Some(0xffe1), base as i32);
        }
        if character.is_ascii_uppercase() {
            return (Some(0xffe1), character.to_ascii_lowercase() as i32);
        }
        if character == ' ' {
            return (None, 0x20);
        }
        if character.is_ascii() {
            return (None, character as i32);
        }
        (None, 0x0100_0000 | character as i32)
    }

    async fn type_text(input: &RemoteInput, text: &str) -> Result<()> {
        for character in text.chars() {
            let (modifier, symbol) = character_keysym(character);
            if let Some(modifier) = modifier {
                input.key_state(modifier, KeyState::Pressed).await?;
            }
            input.key(symbol).await?;
            if let Some(modifier) = modifier {
                input.key_state(modifier, KeyState::Released).await?;
            }
        }
        Ok(())
    }

    async fn ensure_input(input: &mut Option<RemoteInput>) -> Result<&mut RemoteInput> {
        if input.is_none() {
            *input = Some(RemoteInput::connect().await?);
        }
        Ok(input.as_mut().expect("remote input initialized"))
    }

    async fn focus_app(connection: &AccessibilityConnection, wanted: &str) -> Result<()> {
        let app = app_by_name(connection, wanted).await?;
        if let Ok(interfaces) = app.proxies().await {
            if let Ok(component) = interfaces.component().await {
                let _ = component.grab_focus().await?;
            }
        }
        Ok(())
    }

    async fn action(
        connection: &AccessibilityConnection,
        input: &mut Option<RemoteInput>,
        tool: &str,
        args: &Value,
    ) -> Result<()> {
        if tool == "type_text" {
            focus_app(
                connection,
                args.get("app").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            let input = ensure_input(input).await?;
            type_text(
                input,
                args.get("text").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            return Ok(());
        }
        if tool == "press_key" {
            focus_app(
                connection,
                args.get("app").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            let input = ensure_input(input).await?;
            let key = args.get("key").and_then(Value::as_str).unwrap_or_default();
            let parts = key
                .split(['+', ' '])
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            let symbols = parts
                .iter()
                .map(|part| keysym(part).ok_or_else(|| anyhow!("unsupported Linux key: {part}")))
                .collect::<Result<Vec<_>>>()?;
            for symbol in symbols.iter().take(symbols.len().saturating_sub(1)) {
                input.key_state(*symbol, KeyState::Pressed).await?;
            }
            if let Some(symbol) = symbols.last() {
                input.key(*symbol).await?;
            }
            for symbol in symbols.iter().take(symbols.len().saturating_sub(1)).rev() {
                input.key_state(*symbol, KeyState::Released).await?;
            }
            return Ok(());
        }
        if tool == "drag" {
            focus_app(
                connection,
                args.get("app").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            let input = ensure_input(input).await?;
            input
                .drag(
                    args.get("from_x")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("drag.from_x is required"))?,
                    args.get("from_y")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("drag.from_y is required"))?,
                    args.get("to_x")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("drag.to_x is required"))?,
                    args.get("to_y")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("drag.to_y is required"))?,
                )
                .await?;
            return Ok(());
        }
        if tool == "scroll" && args.get("element_index").is_none() {
            focus_app(
                connection,
                args.get("app").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            let input = ensure_input(input).await?;
            let direction = args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down")
                .to_ascii_lowercase();
            let pages = args
                .get("pages")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(1) as i32;
            let (dx, dy) = match direction.as_str() {
                "up" | "u" => (0, -pages),
                "left" | "l" => (-pages, 0),
                "right" | "r" => (pages, 0),
                _ => (0, pages),
            };
            input.scroll(dx, dy).await?;
            return Ok(());
        }
        if tool == "click" && (args.get("x").is_some() || args.get("y").is_some()) {
            focus_app(
                connection,
                args.get("app").and_then(Value::as_str).unwrap_or_default(),
            )
            .await?;
            let input = ensure_input(input).await?;
            let x = args
                .get("x")
                .and_then(Value::as_f64)
                .ok_or_else(|| anyhow!("click.x is required"))?;
            let y = args
                .get("y")
                .and_then(Value::as_f64)
                .ok_or_else(|| anyhow!("click.y is required"))?;
            let button = match args
                .get("mouse_button")
                .and_then(Value::as_str)
                .unwrap_or("left")
            {
                "right" | "r" => 0x111,
                "middle" | "m" => 0x112,
                _ => 0x110,
            };
            input
                .click(
                    x,
                    y,
                    button,
                    args.get("click_count").and_then(Value::as_u64).unwrap_or(1),
                )
                .await?;
            return Ok(());
        }

        let app = app_by_name(
            connection,
            args.get("app").and_then(Value::as_str).unwrap_or_default(),
        )
        .await?;
        let index = args
            .get("element_index")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("native Linux action requires element_index or coordinates"))?
            as usize;
        let mut refs = Vec::new();
        collect_refs(connection, &app, &mut refs).await?;
        let reference = refs
            .get(index)
            .ok_or_else(|| anyhow!("Element index is stale. Call get_app_state again."))?;
        let element = proxy(connection, reference).await?;
        let interfaces = element.proxies().await?;
        match tool {
            "click" => {
                if args.get("x").is_some() || args.get("y").is_some() {
                    let input = ensure_input(input).await?;
                    let x = args
                        .get("x")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("click.x is required"))?;
                    let y = args
                        .get("y")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| anyhow!("click.y is required"))?;
                    let button = match args
                        .get("mouse_button")
                        .and_then(Value::as_str)
                        .unwrap_or("left")
                    {
                        "right" | "r" => 0x111,
                        "middle" | "m" => 0x112,
                        _ => 0x110,
                    };
                    input
                        .click(
                            x,
                            y,
                            button,
                            args.get("click_count").and_then(Value::as_u64).unwrap_or(1),
                        )
                        .await?;
                    return Ok(());
                }
                let action: ActionProxy<'_> = interfaces.action().await?;
                let count = args.get("click_count").and_then(Value::as_u64).unwrap_or(1);
                let mut done = false;
                for i in 0..action.nactions().await? {
                    let name = action.get_name(i).await.unwrap_or_default().to_lowercase();
                    if name == "click" || name == "press" || name == "activate" {
                        for _ in 0..count {
                            done |= action.do_action(i).await?;
                        }
                        break;
                    }
                }
                if !done {
                    bail!("Accessibility element does not expose a click action");
                }
            }
            "perform_secondary_action" => {
                let requested = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                let action: ActionProxy<'_> = interfaces.action().await?;
                for i in 0..action.nactions().await? {
                    if action.get_name(i).await.unwrap_or_default().to_lowercase() == requested
                        && action.do_action(i).await?
                    {
                        return Ok(());
                    }
                }
                bail!("Accessibility action is not exposed by this element");
            }
            "set_value" => {
                let editable: EditableTextProxy<'_> = interfaces.editable_text().await?;
                if !editable
                    .set_text_contents(
                        args.get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .await?
                {
                    bail!("Element rejected text value");
                }
            }
            "select_text" => {
                let text_proxy: TextProxy<'_> = interfaces.text().await?;
                let needle = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("text is required"))?;
                let count = text_proxy.character_count().await?.max(0);
                let haystack = text_proxy.get_text(0, count).await?;
                let prefix = args.get("prefix").and_then(Value::as_str);
                let suffix = args.get("suffix").and_then(Value::as_str);
                let byte_start = haystack
                    .match_indices(needle)
                    .find(|(offset, _)| {
                        let before = &haystack[..*offset];
                        let after = &haystack[*offset + needle.len()..];
                        prefix.map_or(true, |value| before.ends_with(value))
                            && suffix.map_or(true, |value| after.starts_with(value))
                    })
                    .map(|(offset, _)| offset)
                    .ok_or_else(|| anyhow!("text was not found in the accessibility element"))?;
                let start = haystack[..byte_start].chars().count() as i32;
                let end = start + needle.chars().count() as i32;
                let selection_type = args
                    .get("selection_type")
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                let ok = match selection_type {
                    "cursor_before" => text_proxy.set_caret_offset(start).await?,
                    "cursor_after" => text_proxy.set_caret_offset(end).await?,
                    _ => text_proxy.set_selection(0, start, end).await?,
                };
                if !ok {
                    bail!("Accessibility element rejected text selection");
                }
            }
            "scroll" => {
                let component = interfaces.component().await?;
                let (x, y, width, height) = component.get_extents(atspi::CoordType::Screen).await?;
                let input = ensure_input(input).await?;
                input
                    .move_absolute((x + width / 2) as f64, (y + height / 2) as f64)
                    .await?;
                let direction = args
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or("down")
                    .to_ascii_lowercase();
                let pages = args
                    .get("pages")
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    .max(1) as i32;
                let (dx, dy) = match direction.as_str() {
                    "up" | "u" => (0, -pages),
                    "left" | "l" => (-pages, 0),
                    "right" | "r" => (pages, 0),
                    _ => (0, pages),
                };
                input.scroll(dx, dy).await?;
            }
            _ => bail!("unsupported native Linux action: {tool}"),
        }
        Ok(())
    }

    fn collect_refs<'a>(
        connection: &'a AccessibilityConnection,
        node: &'a AccessibleProxy<'a>,
        out: &'a mut Vec<atspi::ObjectRefOwned>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if out.len() >= 600 {
                return Ok(());
            }
            let name = zbus::names::OwnedUniqueName::try_from(
                node.inner().destination().as_str().to_string(),
            )?;
            let path =
                zbus::zvariant::ObjectPath::try_from(node.inner().path().as_str().to_string())?;
            out.push(atspi::ObjectRef::new_owned(name, path));
            for child in node.get_children().await.unwrap_or_default() {
                let child_proxy = proxy(connection, &child).await?;
                collect_refs(connection, &child_proxy, out).await?;
            }
            Ok(())
        })
    }

    fn result(text: impl Into<String>, structured: Option<Value>) -> Value {
        let mut value =
            json!({"content": [{"type": "text", "text": text.into()}], "isError": false});
        if let Some(structured) = structured {
            value["structuredContent"] = structured;
        }
        value
    }

    async fn request(
        connection: &AccessibilityConnection,
        input: &mut Option<RemoteInput>,
        message: &Value,
    ) -> Result<Value> {
        match message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "initialize" => Ok(
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"waku-linux-computer-use","version":"2"}}),
            ),
            "tools/list" => Ok(
                json!({"tools": TOOLS.iter().map(|name| json!({"name":name,"description":"Waku native Linux Computer Use backend.","inputSchema":{"type":"object"}})).collect::<Vec<_>>() }),
            ),
            "ping" | "notifications/initialized" => Ok(json!({})),
            "tools/call" => {
                let params = message
                    .get("params")
                    .ok_or_else(|| anyhow!("missing params"))?;
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let args = params.get("arguments").unwrap_or(&Value::Null);
                match name {
                    "list_apps" => {
                        let apps = list_apps(connection).await?;
                        Ok(result(
                            serde_json::to_string(&apps)?,
                            Some(json!({"apps": apps})),
                        ))
                    }
                    "get_app_state" => {
                        let state = state(
                            connection,
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )
                        .await?;
                        let text = state["text"].as_str().unwrap_or_default().to_string();
                        Ok(result(text, Some(state)))
                    }
                    other => {
                        action(connection, input, other, args).await?;
                        Ok(result("", None))
                    }
                }
            }
            method => bail!("unknown MCP method: {method}"),
        }
    }

    pub fn run() -> Result<()> {
        if std::env::args().nth(1).as_deref() == Some("status") {
            let atspi_ok = smol::block_on(AccessibilityConnection::new()).is_ok();
            let capture_ok = smol::block_on(capture()).is_ok();
            println!(
                "{}",
                json!({"success": atspi_ok, "permissions":{"accessibility":atspi_ok,"screenRecording":capture_ok},"summary":"Native Linux AT-SPI2 and XDG Screenshot backend."})
            );
            return Ok(());
        }
        let connection =
            smol::block_on(AccessibilityConnection::new()).context("connect to the AT-SPI2 bus")?;
        let mut input = None;
        for line in io::stdin().lock().lines() {
            let message: Value = serde_json::from_str(&line?)?;
            if message.get("id").is_none() {
                continue;
            }
            let id = message["id"].clone();
            let response = match smol::block_on(request(&connection, &mut input, &message)) {
                Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
                Err(error) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.to_string()}})
                }
            };
            println!("{}", response);
            io::stdout().flush()?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("waku_computer_use_linux is only available on Linux");
}
