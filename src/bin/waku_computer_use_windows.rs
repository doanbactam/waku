#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

#[cfg(target_os = "windows")]
mod windows_backend {
    use std::io::{self, BufRead, Write};
    use std::mem::size_of;

    use anyhow::{Context, Result, anyhow, bail};
    use base64::Engine as _;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use serde_json::{Value, json};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC, SRCCOPY,
        SelectObject,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
        IUIAutomationInvokePattern, IUIAutomationTextPattern, IUIAutomationTogglePattern,
        IUIAutomationTreeWalker, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
        TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_ExpandCollapsePatternId,
        UIA_InvokePatternId, UIA_TextPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_WHEEL, SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
        SetForegroundWindow,
    };
    use windows::core::{BOOL, BSTR, PWSTR};

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

    #[derive(Clone)]
    struct AppWindow {
        hwnd: HWND,
        name: String,
        image_path: Option<String>,
    }

    struct Backend {
        automation: IUIAutomation,
    }

    fn wide_text(hwnd: HWND) -> String {
        let mut buffer = [0u16; 512];
        let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    fn process_image_path(hwnd: HWND) -> Option<String> {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        }
        let process =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                process,
                Default::default(),
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        unsafe {
            let _ = CloseHandle(process);
        }
        result
            .ok()
            .map(|_| String::from_utf16_lossy(&buffer[..length as usize]))
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, data: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(hwnd).as_bool() } {
            let name = wide_text(hwnd);
            if !name.trim().is_empty() {
                let windows = unsafe { &mut *(data.0 as *mut Vec<AppWindow>) };
                windows.push(AppWindow {
                    hwnd,
                    name,
                    image_path: process_image_path(hwnd),
                });
            }
        }
        BOOL(1)
    }

    fn app_windows() -> Vec<AppWindow> {
        let mut windows = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(enum_window), LPARAM(&mut windows as *mut _ as isize));
        }
        windows
    }

    impl Backend {
        fn new() -> Result<Self> {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED)
                    .ok()
                    .context("initialize COM")?;
            }
            let automation: IUIAutomation =
                unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }?;
            Ok(Self { automation })
        }

        fn find_app(&self, wanted: &str) -> Result<AppWindow> {
            let wanted = wanted.to_ascii_lowercase();
            app_windows()
                .into_iter()
                .find(|app| {
                    let title = app.name.to_ascii_lowercase();
                    let image = app
                        .image_path
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let executable = std::path::Path::new(&image)
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default();
                    title == wanted
                        || title.contains(&wanted)
                        || image == wanted
                        || executable == wanted
                })
                .ok_or_else(|| {
                    anyhow!("application is not available through Windows UIAutomation: {wanted}")
                })
        }

        fn tree_walker(&self) -> Result<IUIAutomationTreeWalker> {
            Ok(unsafe { self.automation.ControlViewWalker()? })
        }

        fn collect(
            &self,
            node: &IUIAutomationElement,
            out: &mut Vec<(IUIAutomationElement, Value)>,
        ) -> Result<()> {
            if out.len() >= 600 {
                return Ok(());
            }
            let name = unsafe { node.CurrentName() }
                .map(|value| value.to_string())
                .unwrap_or_default();
            let control_type = unsafe { node.CurrentControlType() }
                .map(|value| value.0.to_string())
                .unwrap_or_else(|_| "unknown".into());
            let enabled = unsafe { node.CurrentIsEnabled() }
                .map(|value| value.as_bool())
                .unwrap_or(false);
            let bounds = unsafe { node.CurrentBoundingRectangle() }.ok().map(|rect| {
                json!([
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top
                ])
            });
            let mut item = json!({"index": out.len(), "role": control_type, "name": name, "description": "", "enabled": enabled});
            if let Some(bounds) = bounds {
                item["bounds"] = bounds;
            }
            out.push((node.clone(), item));
            let walker = self.tree_walker()?;
            let mut child = unsafe { walker.GetFirstChildElement(node) }.ok();
            while let Some(current) = child {
                self.collect(&current, out)?;
                if out.len() >= 600 {
                    break;
                }
                child = unsafe { walker.GetNextSiblingElement(&current) }.ok();
            }
            Ok(())
        }

        fn elements(&self, app: &AppWindow) -> Result<Vec<(IUIAutomationElement, Value)>> {
            let root = unsafe { self.automation.ElementFromHandle(app.hwnd)? };
            let mut elements = Vec::new();
            self.collect(&root, &mut elements)?;
            Ok(elements)
        }

        fn screenshot(&self) -> Result<String> {
            let screen = unsafe { GetDC(None) };
            if screen.0.is_null() {
                bail!("GetDC failed");
            }
            let width = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX(0),
                )
            };
            let height = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX(1),
                )
            };
            let memory = unsafe { CreateCompatibleDC(Some(screen)) };
            let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
            if memory.0.is_null() || bitmap.0.is_null() {
                unsafe {
                    ReleaseDC(None, screen);
                }
                bail!("create screen bitmap failed");
            }
            let previous = unsafe { SelectObject(memory, HGDIOBJ(bitmap.0)) };
            unsafe {
                BitBlt(memory, 0, 0, width, height, Some(screen), 0, 0, SRCCOPY)?;
            }
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    ..Default::default()
                },
                ..Default::default()
            };
            unsafe {
                GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    Some(pixels.as_mut_ptr().cast()),
                    &mut info,
                    DIB_RGB_COLORS,
                );
            }
            unsafe {
                SelectObject(memory, previous);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(memory);
                ReleaseDC(None, screen);
            }
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let image = ImageBuffer::<Rgba<u8>, _>::from_raw(width as u32, height as u32, pixels)
                .ok_or_else(|| anyhow!("invalid screen bitmap"))?;
            let mut output = std::io::Cursor::new(Vec::new());
            image.write_to(&mut output, ImageFormat::Png)?;
            Ok(format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(output.into_inner())
            ))
        }
    }

    fn send_input(input: INPUT) -> Result<()> {
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        if sent != 1 {
            bail!("SendInput failed");
        }
        Ok(())
    }

    fn input_key(vk: u16, key_up: bool) -> Result<()> {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if key_up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send_input(input)
    }

    fn input_unicode(unit: u16, key_up: bool) -> Result<()> {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE
                        | if key_up {
                            KEYEVENTF_KEYUP
                        } else {
                            KEYBD_EVENT_FLAGS(0)
                        },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send_input(input)
    }

    fn input_mouse(
        flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
        data: u32,
    ) -> Result<()> {
        let input = INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: windows::Win32::UI::Input::KeyboardAndMouse::MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send_input(input)
    }

    fn mouse_button(
        name: &str,
    ) -> (
        windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
        windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    ) {
        match name.to_ascii_lowercase().as_str() {
            "right" | "r" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" | "m" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        }
    }

    fn click_at(x: i32, y: i32, button: &str, count: u64) -> Result<()> {
        if !(1..=10).contains(&count) {
            bail!("click_count must be an integer from 1 through 10");
        }
        unsafe {
            SetCursorPos(x, y)?;
        }
        let (down, up) = mouse_button(button);
        for _ in 0..count {
            input_mouse(down, 0)?;
            input_mouse(up, 0)?;
        }
        Ok(())
    }

    fn focus_window(hwnd: HWND) -> Result<()> {
        if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
            bail!("could not focus target Windows application");
        }
        Ok(())
    }

    fn select_text(
        element: &IUIAutomationElement,
        text: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        selection_type: &str,
    ) -> Result<()> {
        let pattern: IUIAutomationTextPattern =
            unsafe { element.GetCurrentPatternAs(UIA_TextPatternId)? };
        let document = unsafe { pattern.DocumentRange()? };
        let contents = unsafe { document.GetText(-1)? }.to_string();
        let byte_start = contents
            .match_indices(text)
            .find(|(offset, _)| {
                let before = &contents[..*offset];
                let after = &contents[*offset + text.len()..];
                prefix.map_or(true, |value| before.ends_with(value))
                    && suffix.map_or(true, |value| after.starts_with(value))
            })
            .map(|(offset, _)| offset)
            .ok_or_else(|| anyhow!("text was not found in the accessibility element"))?;
        let start = contents[..byte_start].chars().count() as i32;
        let length = text.chars().count() as i32;
        let range = unsafe { document.Clone()? };
        unsafe {
            let cursor_offset = if selection_type == "cursor_after" {
                start + length
            } else {
                start
            };
            range.MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                cursor_offset,
            )?;
            if matches!(selection_type, "cursor_before" | "cursor_after") {
                range.MoveEndpointByRange(
                    TextPatternRangeEndpoint_End,
                    &range,
                    TextPatternRangeEndpoint_Start,
                )?;
            } else {
                range.MoveEndpointByUnit(
                    TextPatternRangeEndpoint_End,
                    TextUnit_Character,
                    length,
                )?;
            }
            range.Select()?;
        }
        Ok(())
    }

    fn secondary_action(element: &IUIAutomationElement, action: &str) -> Result<()> {
        match action
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '_', '-'], "")
            .as_str()
        {
            "click" | "press" | "activate" | "invoke" | "showmenu" => {
                let pattern: IUIAutomationInvokePattern =
                    unsafe { element.GetCurrentPatternAs(UIA_InvokePatternId)? };
                unsafe {
                    pattern.Invoke()?;
                }
            }
            "expand" | "expandcollapse" => {
                let pattern: IUIAutomationExpandCollapsePattern =
                    unsafe { element.GetCurrentPatternAs(UIA_ExpandCollapsePatternId)? };
                unsafe {
                    pattern.Expand()?;
                }
            }
            "collapse" => {
                let pattern: IUIAutomationExpandCollapsePattern =
                    unsafe { element.GetCurrentPatternAs(UIA_ExpandCollapsePatternId)? };
                unsafe {
                    pattern.Collapse()?;
                }
            }
            "toggle" => {
                let pattern: IUIAutomationTogglePattern =
                    unsafe { element.GetCurrentPatternAs(UIA_TogglePatternId)? };
                unsafe {
                    pattern.Toggle()?;
                }
            }
            other => bail!("unsupported Windows UI Automation action: {other}"),
        }
        Ok(())
    }

    fn virtual_key(name: &str) -> Result<u16> {
        let key = name.rsplit('+').next().unwrap_or(name).to_ascii_lowercase();
        let value = match key.as_str() {
            "return" | "enter" => 0x0d,
            "tab" => 0x09,
            "escape" | "esc" => 0x1b,
            "backspace" => 0x08,
            "delete" | "del" => 0x2e,
            "space" => 0x20,
            "left" => 0x25,
            "up" => 0x26,
            "right" => 0x27,
            "down" => 0x28,
            "home" => 0x24,
            "end" => 0x23,
            "pageup" => 0x21,
            "pagedown" => 0x22,
            "shift" => 0x10,
            "ctrl" | "control" => 0x11,
            "alt" => 0x12,
            "super" | "meta" | "win" => 0x5b,
            single if single.len() == 1 => single.as_bytes()[0].to_ascii_uppercase() as u16,
            f if f.len() > 1 && f.starts_with('f') => f[1..]
                .parse::<u16>()
                .ok()
                .filter(|n| (1..=24).contains(n))
                .map(|n| 0x6f + n)
                .ok_or_else(|| anyhow!("unknown key: {name}"))?,
            _ => bail!("unknown key: {name}"),
        };
        Ok(value)
    }

    fn press_key(name: &str) -> Result<()> {
        let modifiers = name
            .split('+')
            .take(name.split('+').count().saturating_sub(1))
            .map(virtual_key)
            .collect::<Result<Vec<_>>>()?;
        let key = virtual_key(name)?;
        for modifier in &modifiers {
            input_key(*modifier, false)?;
        }
        input_key(key, false)?;
        input_key(key, true)?;
        for modifier in modifiers.iter().rev() {
            input_key(*modifier, true)?;
        }
        Ok(())
    }

    fn result(text: impl Into<String>, structured: Option<Value>) -> Value {
        let mut value = json!({"content":[{"type":"text","text":text.into()}],"isError":false});
        if let Some(structured) = structured {
            value["structuredContent"] = structured;
        }
        value
    }

    fn element_center(item: &Value) -> Result<(i32, i32)> {
        let bounds = item
            .get("bounds")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("element has no screen bounds"))?;
        let x = bounds
            .first()
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("element bounds are invalid"))?;
        let y = bounds
            .get(1)
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("element bounds are invalid"))?;
        let width = bounds.get(2).and_then(Value::as_i64).unwrap_or(0);
        let height = bounds.get(3).and_then(Value::as_i64).unwrap_or(0);
        Ok(((x + width / 2) as i32, (y + height / 2) as i32))
    }

    fn request(backend: &Backend, message: &Value) -> Result<Value> {
        match message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "initialize" => Ok(
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"waku-windows-computer-use","version":"1"}}),
            ),
            "tools/list" => Ok(
                json!({"tools": TOOLS.iter().map(|name| json!({"name":name,"description":"Waku native Windows Computer Use backend.","inputSchema":{"type":"object"}})).collect::<Vec<_>>() }),
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
                        let apps = app_windows().into_iter().map(|app| json!({"id":app.name,"displayName":app.name,"isRunning":true})).collect::<Vec<_>>();
                        Ok(result(
                            serde_json::to_string(&apps)?,
                            Some(json!({"apps":apps})),
                        ))
                    }
                    "get_app_state" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        let elements = backend.elements(&app)?;
                        let text = serde_json::to_string(
                            &elements.iter().map(|(_, item)| item).collect::<Vec<_>>(),
                        )?;
                        Ok(result(
                            text.clone(),
                            Some(
                                json!({"app":app.name,"screenshot":{"url":backend.screenshot()?},"text":text}),
                            ),
                        ))
                    }
                    "press_key" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        focus_window(app.hwnd)?;
                        let key = args
                            .get("key")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow!("Windows press_key requires a key name"))?;
                        press_key(key)?;
                        Ok(result("", None))
                    }
                    "type_text" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        focus_window(app.hwnd)?;
                        for character in args
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .encode_utf16()
                        {
                            input_unicode(character, false)?;
                            input_unicode(character, true)?;
                        }
                        Ok(result("", None))
                    }
                    "click" => {
                        if let Some(raw_index) = args.get("element_index").and_then(Value::as_u64) {
                            let app = backend.find_app(
                                args.get("app").and_then(Value::as_str).unwrap_or_default(),
                            )?;
                            focus_window(app.hwnd)?;
                            let elements = backend.elements(&app)?;
                            let element = elements
                                .get(raw_index as usize)
                                .ok_or_else(|| {
                                    anyhow!("Element index is stale. Call get_app_state again.")
                                })?
                                .0
                                .clone();
                            let button = args
                                .get("mouse_button")
                                .and_then(Value::as_str)
                                .unwrap_or("left");
                            let invoke: IUIAutomationInvokePattern =
                                unsafe { element.GetCurrentPatternAs(UIA_InvokePatternId)? };
                            let count = args
                                .get("click_count")
                                .and_then(Value::as_u64)
                                .unwrap_or(1)
                                .clamp(1, 10);
                            if matches!(button.to_ascii_lowercase().as_str(), "left" | "l")
                                && count == 1
                            {
                                unsafe {
                                    invoke.Invoke()?;
                                }
                            } else {
                                let (x, y) = element_center(&elements[raw_index as usize].1)?;
                                click_at(x, y, button, count)?;
                            }
                            Ok(result("", None))
                        } else {
                            let app = backend.find_app(
                                args.get("app").and_then(Value::as_str).unwrap_or_default(),
                            )?;
                            focus_window(app.hwnd)?;
                            let x = args
                                .get("x")
                                .and_then(Value::as_i64)
                                .ok_or_else(|| anyhow!("click.x is required"))?
                                as i32;
                            let y = args
                                .get("y")
                                .and_then(Value::as_i64)
                                .ok_or_else(|| anyhow!("click.y is required"))?
                                as i32;
                            click_at(
                                x,
                                y,
                                args.get("mouse_button")
                                    .and_then(Value::as_str)
                                    .unwrap_or("left"),
                                args.get("click_count").and_then(Value::as_u64).unwrap_or(1),
                            )?;
                            Ok(result("", None))
                        }
                    }
                    "drag" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        focus_window(app.hwnd)?;
                        let x = args
                            .get("from_x")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| anyhow!("drag.from_x is required"))?
                            as i32;
                        let y = args
                            .get("from_y")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| anyhow!("drag.from_y is required"))?
                            as i32;
                        let to_x = args
                            .get("to_x")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| anyhow!("drag.to_x is required"))?
                            as i32;
                        let to_y = args
                            .get("to_y")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| anyhow!("drag.to_y is required"))?
                            as i32;
                        unsafe {
                            SetCursorPos(x, y)?;
                        }
                        input_mouse(MOUSEEVENTF_LEFTDOWN, 0)?;
                        unsafe {
                            SetCursorPos(to_x, to_y)?;
                        }
                        input_mouse(MOUSEEVENTF_LEFTUP, 0)?;
                        Ok(result("", None))
                    }
                    "scroll" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        focus_window(app.hwnd)?;
                        let (x, y) = if let Some(index) =
                            args.get("element_index").and_then(Value::as_u64)
                        {
                            let app = backend.find_app(
                                args.get("app").and_then(Value::as_str).unwrap_or_default(),
                            )?;
                            let elements = backend.elements(&app)?;
                            element_center(
                                &elements
                                    .get(index as usize)
                                    .ok_or_else(|| {
                                        anyhow!("Element index is stale. Call get_app_state again.")
                                    })?
                                    .1,
                            )?
                        } else {
                            (
                                args.get("x").and_then(Value::as_i64).unwrap_or(0) as i32,
                                args.get("y").and_then(Value::as_i64).unwrap_or(0) as i32,
                            )
                        };
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
                        let (flags, delta) = if matches!(direction.as_str(), "up" | "u") {
                            (MOUSEEVENTF_WHEEL, pages * 120)
                        } else if matches!(direction.as_str(), "down" | "d") {
                            (MOUSEEVENTF_WHEEL, -pages * 120)
                        } else if matches!(direction.as_str(), "left" | "l") {
                            (MOUSEEVENTF_HWHEEL, -pages * 120)
                        } else {
                            (MOUSEEVENTF_HWHEEL, pages * 120)
                        };
                        unsafe {
                            SetCursorPos(x, y)?;
                        }
                        input_mouse(flags, delta as u32)?;
                        Ok(result("", None))
                    }
                    "perform_secondary_action" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        let elements = backend.elements(&app)?;
                        let index = args
                            .get("element_index")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow!("element_index is required"))?
                            as usize;
                        let element = &elements
                            .get(index)
                            .ok_or_else(|| anyhow!("element_index is out of range"))?
                            .0;
                        secondary_action(
                            element,
                            args.get("action")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )?;
                        Ok(result("", None))
                    }
                    "set_value" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        let elements = backend.elements(&app)?;
                        let index = args
                            .get("element_index")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow!("element_index is required"))?
                            as usize;
                        let element = elements
                            .get(index)
                            .ok_or_else(|| anyhow!("element_index is out of range"))?
                            .0
                            .clone();
                        let value = args
                            .get("value")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow!("value is required"))?;
                        let pattern: IUIAutomationValuePattern =
                            unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId)? };
                        let value = BSTR::from(value);
                        unsafe {
                            pattern.SetValue(&value)?;
                        }
                        Ok(result("", None))
                    }
                    "select_text" => {
                        let app = backend.find_app(
                            args.get("app").and_then(Value::as_str).unwrap_or_default(),
                        )?;
                        let elements = backend.elements(&app)?;
                        let index = args
                            .get("element_index")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow!("element_index is required"))?
                            as usize;
                        let element = elements
                            .get(index)
                            .ok_or_else(|| anyhow!("element_index is out of range"))?
                            .0
                            .clone();
                        let text = args
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow!("text is required"))?;
                        select_text(
                            &element,
                            text,
                            args.get("prefix").and_then(Value::as_str),
                            args.get("suffix").and_then(Value::as_str),
                            args.get("selection_type")
                                .and_then(Value::as_str)
                                .unwrap_or("text"),
                        )?;
                        Ok(result("", None))
                    }
                    other => bail!("unsupported native Windows action: {other}"),
                }
            }
            method => bail!("unknown MCP method: {method}"),
        }
    }

    pub fn run() -> Result<()> {
        let backend = Backend::new()?;
        if std::env::args().nth(1).as_deref() == Some("status") {
            let screenshot = backend.screenshot().is_ok();
            println!(
                "{}",
                json!({"success":true,"permissions":{"accessibility":true,"screenRecording":screenshot},"summary":"Native Windows UIAutomation and GDI capture backend."})
            );
            return Ok(());
        }
        for line in io::stdin().lock().lines() {
            let message: Value = serde_json::from_str(&line?)?;
            if message.get("id").is_none() {
                continue;
            }
            let id = message["id"].clone();
            let response = match request(&backend, &message) {
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

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    windows_backend::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("waku_computer_use_windows is only available on Windows");
}
