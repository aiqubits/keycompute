//! ECharts JS 互调绑定层
//!
//! 通过 `js_sys` / `web_sys` 直接调用全局 `echarts` 对象的 API，
//! 替代 charming 的 WasmRenderer 中间层。

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// 在指定 DOM 容器中初始化或获取 ECharts 实例并设置 option
///
/// - 若容器已存在 ECharts 实例则复用（避免重复 init 导致警告）
/// - option 参数为 `serde_json::Value`，内部转换为 JS 对象传递
#[cfg(target_arch = "wasm32")]
pub fn render_chart(container_id: &str, width: u32, height: u32, option: &serde_json::Value) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(dom) = document.get_element_by_id(container_id) else {
        return;
    };

    // 获取全局 echarts 对象
    let Ok(echarts) = js_sys::Reflect::get(&window, &"echarts".into()) else {
        return;
    };
    if echarts.is_undefined() || echarts.is_null() {
        return;
    }

    // 尝试获取已有实例（避免重复 init）
    let instance = get_or_init_instance(&echarts, &dom, width, height);
    let Some(instance) = instance else {
        return;
    };

    // 将 serde_json::Value 转为 JsValue
    let option_str = serde_json::to_string(option).unwrap_or_default();
    let Ok(option_js) = js_sys::JSON::parse(&option_str) else {
        return;
    };

    // 调用 instance.setOption(option, true) — 第二参数 true 表示 notMerge
    let Ok(set_option_fn) = js_sys::Reflect::get(&instance, &"setOption".into()) else {
        return;
    };
    let Some(f) = set_option_fn.dyn_ref::<js_sys::Function>() else {
        return;
    };
    let _ = f.call2(&instance, &option_js, &wasm_bindgen::JsValue::TRUE);
}

/// 获取或初始化 ECharts 实例
#[cfg(target_arch = "wasm32")]
fn get_or_init_instance(
    echarts: &wasm_bindgen::JsValue,
    dom: &web_sys::Element,
    width: u32,
    height: u32,
) -> Option<wasm_bindgen::JsValue> {
    // echarts.getInstanceByDom(dom)
    let get_fn = js_sys::Reflect::get(echarts, &"getInstanceByDom".into()).ok()?;
    let existing = get_fn
        .dyn_ref::<js_sys::Function>()?
        .call1(echarts, dom)
        .ok();

    if let Some(inst) = existing.filter(|inst| !inst.is_undefined() && !inst.is_null()) {
        // 复用已有实例，调整尺寸
        if let Some(f) = js_sys::Reflect::get(&inst, &"resize".into())
            .ok()
            .and_then(|resize_fn| resize_fn.dyn_ref::<js_sys::Function>().cloned())
        {
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&opts, &"width".into(), &(width as f64).into());
            let _ = js_sys::Reflect::set(&opts, &"height".into(), &(height as f64).into());
            let _ = f.call1(&inst, &opts);
        }
        return Some(inst);
    }

    // echarts.init(dom, null, { width, height })
    let init_fn = js_sys::Reflect::get(echarts, &"init".into()).ok()?;
    let init_func = init_fn.dyn_ref::<js_sys::Function>()?;

    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"width".into(), &(width as f64).into());
    let _ = js_sys::Reflect::set(&opts, &"height".into(), &(height as f64).into());

    init_func
        .call3(echarts, dom, &wasm_bindgen::JsValue::NULL, &opts)
        .ok()
}

/// 销毁指定容器的 ECharts 实例（组件卸载时调用）
#[cfg(target_arch = "wasm32")]
pub fn dispose_chart(container_id: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(dom) = document.get_element_by_id(container_id) else {
        return;
    };

    let Ok(echarts) = js_sys::Reflect::get(&window, &"echarts".into()) else {
        return;
    };
    if echarts.is_undefined() || echarts.is_null() {
        return;
    }

    // echarts.getInstanceByDom(dom)?.dispose()
    let Ok(get_fn) = js_sys::Reflect::get(&echarts, &"getInstanceByDom".into()) else {
        return;
    };
    let Some(f) = get_fn.dyn_ref::<js_sys::Function>() else {
        return;
    };
    let Ok(instance) = f.call1(&echarts, &dom) else {
        return;
    };
    if instance.is_undefined() || instance.is_null() {
        return;
    }

    let Ok(dispose_fn) = js_sys::Reflect::get(&instance, &"dispose".into()) else {
        return;
    };
    let Some(dispose) = dispose_fn.dyn_ref::<js_sys::Function>() else {
        return;
    };
    let _ = dispose.call0(&instance);
}
