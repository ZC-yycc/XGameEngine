//! 脚本层的 UI 方法（`GameCtx` 的 `ui_*` 一族）。
//!
//! # 为什么单独一个文件，而不是继续堆在 `ctx.rs`
//!
//! UI 有二十多个方法，全塞进 `ctx.rs` 会让那个文件从"状态束 + 少量工具"
//! 变成一锅粥。这里只依赖 `self.scene`（UI 组件与运行时标志都在场景里），
//! 不碰物理 / 音频 / 命令缓冲，天然是一块可以独立阅读的能力面。
//!
//! 真相源仍然是 `GameCtx` 的方法层（`lua_api.rs` / `ui_api.rs` 只是转发壳），
//! 与其余 API 的约定一致。
//!
//! # 颜色约定
//!
//! 脚本给的是 **sRGB 0..1**（美术 / 设计师的直觉：`0.1` 就是深灰）。
//! 引擎内部 `Color` 是线性空间，所以入口统一 `to_linear()` 一次 ——
//! 这段转换只此一处，别在调用点各写一遍。
//!
//! # 坐标约定
//!
//! 锚点是**父矩形的归一化坐标**（0..1，允许溢出），偏移是**像素**。
//! 与场景文件里的 `UiElement` 字段一一对应，见 `xgame_core::ui` 的模块文档。

use xgame_core::{Color, Entity, EntityId, UiAlign, UiElement, UiKind};

use crate::context::ScriptCommand;
use crate::ctx::GameCtx;

/// sRGB 0..1 → 线性 RGBA 数组（`UiStyle` 的字段格式）。
fn color_srgb(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    Color { r, g, b, a }.to_linear().to_array()
}

impl GameCtx<'_> {
    /// 取一个 UI 元素（不存在 / 没挂 UI 时返回 `None`）。
    fn ui_element(&mut self, id: EntityId) -> Option<&mut UiElement> {
        let index = self.scene.find_entity(id)?;
        self.scene.entities.get_mut(index)?.ui.as_mut()
    }

    /// 目标实体是不是 UI 元素。
    pub fn ui_has(&mut self, id: EntityId) -> bool {
        self.ui_element(id).is_some()
    }

    /// 改文本内容。
    pub fn ui_set_text(&mut self, id: EntityId, text: &str) {
        if let Some(ui) = self.ui_element(id) {
            ui.text.content.clear();
            ui.text.content.push_str(text);
        }
    }

    /// 读文本内容（不存在时返回空串）。
    pub fn ui_text(&mut self, id: EntityId) -> String {
        self.ui_element(id)
            .map(|ui| ui.text.content.clone())
            .unwrap_or_default()
    }

    /// 显示 / 隐藏（隐藏会连带隐藏整棵子树）。
    pub fn ui_set_visible(&mut self, id: EntityId, visible: bool) {
        if let Some(ui) = self.ui_element(id) {
            ui.visible = visible;
        }
    }

    /// 是否可见（不判断父节点）。
    pub fn ui_visible(&mut self, id: EntityId) -> bool {
        self.ui_element(id).map(|ui| ui.visible).unwrap_or(false)
    }

    /// 启用 / 禁用交互。禁用态只对按钮有视觉反馈（半透明）。
    pub fn ui_set_enabled(&mut self, id: EntityId, enabled: bool) {
        if let Some(ui) = self.ui_element(id) {
            ui.interactable = enabled;
        }
    }

    /// 是否接受指针事件。
    pub fn ui_enabled(&mut self, id: EntityId) -> bool {
        self.ui_element(id)
            .map(|ui| ui.interactable)
            .unwrap_or(false)
    }

    /// 背景色（sRGB 0..1）。
    pub fn ui_set_bg_color(&mut self, id: EntityId, r: f32, g: f32, b: f32, a: f32) {
        if let Some(ui) = self.ui_element(id) {
            ui.style.background = color_srgb(r, g, b, a);
        }
    }

    /// 文字颜色（sRGB 0..1）。
    pub fn ui_set_text_color(&mut self, id: EntityId, r: f32, g: f32, b: f32, a: f32) {
        if let Some(ui) = self.ui_element(id) {
            ui.text.color = color_srgb(r, g, b, a);
        }
    }

    /// 字号（像素）。
    pub fn ui_set_text_size(&mut self, id: EntityId, size: f32) {
        if let Some(ui) = self.ui_element(id) {
            ui.text.size = size.clamp(1.0, 512.0);
        }
    }

    /// 水平对齐：`"left"` / `"center"` / `"right"`（其他值忽略）。
    pub fn ui_set_text_align(&mut self, id: EntityId, align: &str) {
        let align = match align.to_ascii_lowercase().as_str() {
            "left" => UiAlign::Left,
            "center" | "centre" | "middle" => UiAlign::Center,
            "right" => UiAlign::Right,
            _ => return,
        };
        if let Some(ui) = self.ui_element(id) {
            ui.text.align = align;
        }
    }

    /// 圆角半径（像素）。
    pub fn ui_set_radius(&mut self, id: EntityId, radius: f32) {
        if let Some(ui) = self.ui_element(id) {
            ui.style.corner_radius = radius.max(0.0);
        }
    }

    /// 一次设定锚点与偏移（四个两元组）。
    #[allow(clippy::too_many_arguments)]
    pub fn ui_set_rect(
        &mut self,
        id: EntityId,
        anchor_min: [f32; 2],
        anchor_max: [f32; 2],
        offset_min: [f32; 2],
        offset_max: [f32; 2],
    ) {
        if let Some(ui) = self.ui_element(id) {
            ui.anchor_min = anchor_min;
            ui.anchor_max = anchor_max;
            ui.offset_min = offset_min;
            ui.offset_max = offset_max;
        }
    }

    /// 只改锚点。
    pub fn ui_set_anchor(&mut self, id: EntityId, anchor_min: [f32; 2], anchor_max: [f32; 2]) {
        if let Some(ui) = self.ui_element(id) {
            ui.anchor_min = anchor_min;
            ui.anchor_max = anchor_max;
        }
    }

    /// 只改偏移。
    pub fn ui_set_offset(&mut self, id: EntityId, offset_min: [f32; 2], offset_max: [f32; 2]) {
        if let Some(ui) = self.ui_element(id) {
            ui.offset_min = offset_min;
            ui.offset_max = offset_max;
        }
    }

    /// 改尺寸，保持 `offset_min`（左上角）不动。
    pub fn ui_set_size(&mut self, id: EntityId, width: f32, height: f32) {
        if let Some(ui) = self.ui_element(id) {
            ui.offset_max = [ui.offset_min[0] + width, ui.offset_min[1] + height];
        }
    }

    /// 改位置（相对父矩形左上角的像素偏移），保持尺寸。
    pub fn ui_set_pos(&mut self, id: EntityId, x: f32, y: f32) {
        if let Some(ui) = self.ui_element(id) {
            let size = ui.size();
            ui.offset_min = [x, y];
            ui.offset_max = [x + size[0], y + size[1]];
        }
    }

    /// 换图片（项目相对路径）。
    pub fn ui_set_image(&mut self, id: EntityId, path: &str) {
        if let Some(ui) = self.ui_element(id) {
            let image = ui.image.get_or_insert_with(Default::default);
            image.path.clear();
            image.path.push_str(path);
        }
    }

    /// 图片着色（sRGB 0..1，白色 = 原图）。
    pub fn ui_set_image_tint(&mut self, id: EntityId, r: f32, g: f32, b: f32, a: f32) {
        if let Some(ui) = self.ui_element(id) {
            if let Some(image) = ui.image.as_mut() {
                image.tint = color_srgb(r, g, b, a);
            }
        }
    }

    /// 填充比例 `0..1`（血条 / 进度条：从左往右裁剪）。
    pub fn ui_set_fill(&mut self, id: EntityId, fill: f32) {
        if let Some(ui) = self.ui_element(id) {
            if let Some(image) = ui.image.as_mut() {
                image.fill = fill.clamp(0.0, 1.0);
            }
        }
    }

    /// 本帧是否被点击（在元素上按下并在元素上松开）。
    pub fn ui_clicked(&mut self, id: EntityId) -> bool {
        self.ui_element(id)
            .map(|ui| ui.state.clicked)
            .unwrap_or(false)
    }

    /// 本帧是否悬停。
    pub fn ui_hovered(&mut self, id: EntityId) -> bool {
        self.ui_element(id)
            .map(|ui| ui.state.hovered)
            .unwrap_or(false)
    }

    /// 焦点是否在这个元素上（方向键 / 十字键 / 左摇杆 / 鼠标按下都会搬焦点）。
    pub fn ui_focused(&mut self, id: EntityId) -> bool {
        self.ui_element(id)
            .map(|ui| ui.state.focused)
            .unwrap_or(false)
    }

    /// 把焦点交给这个元素（菜单一打开就选中默认项）。
    ///
    /// # 三条语义
    ///
    /// 1. **只认能按的东西**：目标必须是 `kind` 为 Button、可见且可交互的元素。
    ///    否则**整个调用无效**（不搬焦点、不留半截状态）—— 与"焦点只落在按钮上"
    ///    的导航准入条件是同一条规则，两处不一致就会造出"脚本能选中、方向键选不中"
    ///    的幽灵焦点。
    /// 2. **互斥**：会把其它元素的焦点标志一起清掉。焦点天然唯一，允许两个元素
    ///    同时说自己有焦点只会让画面出现两圈环。
    /// 3. **下一帧生效**：真正的焦点表在 `World` 上，脚本改不到；这里写场景标志
    ///    并登记一条请求，运行时在帧末结算 —— 与「脚本改 UI 矩形下一帧才参与命中」
    ///    的时序一致（见 `Docs/决策记录.md` §五十二.4）。**同帧**读 `ui_focused`
    ///    已经为真（标志先写），但**方向键的出发点**要等下一帧。
    pub fn ui_set_focus(&mut self, id: EntityId) {
        let index = match self.scene.find_entity(id) {
            Some(index) => index,
            None => return,
        };
        // 先判能不能聚焦，再动任何状态：不可聚焦的调用必须是彻底的 no-op。
        let can_focus = self.scene.entities[index]
            .ui
            .as_ref()
            .is_some_and(|ui| ui.kind == UiKind::Button && ui.visible && ui.interactable);
        if !can_focus {
            return;
        }
        self.clear_focus_flags(Some(id));
        if let Some(ui) = self.scene.entities[index].ui.as_mut() {
            ui.state.focused = true;
        }
        // 直接写缓冲（与 `ui_spawn` 同款）：`GameCtx::push_command` 是 `ctx.rs` 的
        // 私有方法，同一个 crate 的两个模块之间不共享它。
        self.commands.push(ScriptCommand::UiFocus { target: Some(id) });
    }

    /// 清空焦点：下一次推方向键会重新选第一个。
    pub fn ui_clear_focus(&mut self) {
        self.clear_focus_flags(None);
        self.commands.push(ScriptCommand::UiFocus { target: None });
    }

    /// 清掉所有元素的焦点标志（`keep` 指向的那个除外，`None` = 全清）。
    ///
    /// 逐实体扫一遍是 O(n)，但焦点操作发生在"开菜单 / 切页"这种低频时刻，
    /// 一帧至多几次 —— 不为此在 `Scene` 上再挂一张反向索引（那张表还得跟着
    /// 增删实体同步，代价远大于收益）。
    fn clear_focus_flags(&mut self, keep: Option<EntityId>) {
        for entity in &mut self.scene.entities {
            if Some(entity.id) == keep {
                continue;
            }
            if let Some(ui) = entity.ui.as_mut() {
                ui.state.focused = false;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 运行时创建
    // -----------------------------------------------------------------------

    /// 生成一个 UI 元素并挂到 `parent` 下（`None` = 屏幕根）。
    fn ui_spawn(&mut self, name: &str, element: UiElement, parent: Option<EntityId>) -> EntityId {
        let id = EntityId::new();
        let mut entity = Entity::new(name);
        entity.id = id;
        entity.ui = Some(element);
        entity.parent = parent;
        let identity = xgame_core::RuntimeEntityIdentity::from_legacy(&entity).ok();
        self.scene.entities.push(entity);
        self.commands.push(ScriptCommand::EntitySpawned {
            id,
            physics: None,
            identity,
        });
        id
    }

    /// 生成一段文本，返回实体 ID。
    pub fn ui_spawn_label(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        parent: Option<EntityId>,
    ) -> EntityId {
        let mut element = UiElement::text(text);
        element.text.size = size.clamp(1.0, 512.0);
        // 文本宽度按"一个汉字一个字号的宽度"估：够用，且不需要字体度量
        // （脚本层不该为了摆个标签去加载字体）。
        let width = (text.chars().count() as f32 * size).max(size);
        let element = element.at([0.0, 0.0], x, y, width, size * 1.4);
        self.ui_spawn("Label", element, parent)
    }

    /// 生成一个按钮，返回实体 ID。
    pub fn ui_spawn_button(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        parent: Option<EntityId>,
    ) -> EntityId {
        let element = UiElement::button(text).at([0.0, 0.0], x, y, width, height);
        self.ui_spawn("Button", element, parent)
    }

    /// 生成一个面板（纯色块），返回实体 ID。
    pub fn ui_spawn_panel(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        parent: Option<EntityId>,
    ) -> EntityId {
        let mut element = UiElement::panel(width, height).at([0.0, 0.0], x, y, width, height);
        element.kind = UiKind::Panel;
        self.ui_spawn("Panel", element, parent)
    }

    /// 生成一个图片元素（血条 / 图标），返回实体 ID。
    pub fn ui_spawn_image(
        &mut self,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        parent: Option<EntityId>,
    ) -> EntityId {
        let mut element = UiElement::image(path).at([0.0, 0.0], x, y, width, height);
        element.image.as_mut().unwrap().keep_aspect = false;
        self.ui_spawn("Image", element, parent)
    }
}
