// editor_input.rs - 编辑器输入系统

use egui::{Key, PointerButton, Pos2};

/// 输入事件类型
#[derive(Debug, Clone)]
pub enum EditorInputEvent {
    /// 按键按下（单次触发）
    KeyPressed(Key),
    /// 按键释放
    KeyReleased(Key),
    /// 按键按住（持续触发）
    KeyHeld(Key),
    
    /// 鼠标按钮按下
    MousePressed(PointerButton),
    /// 鼠标按钮释放
    MouseReleased(PointerButton),
    /// 鼠标移动
    MouseMoved(egui::Vec2),
    /// 滚轮
    MouseWheel(f32),
    
    /// 拖拽开始
    DragStarted(PointerButton),
    /// 拖拽进行中
    DragDelta(egui::Vec2),
    /// 拖拽释放
    DragReleased(PointerButton),
}

/// 输入上下文 - 定义输入的作用域
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputContext {
    /// 全局 - 任何地方都响应
    Global,
    /// 视口内 - 仅在 3D 视口中响应
    Viewport,
    /// 视口外 - 仅在 UI 面板中响应
    UI,
}

/// 输入动作 - 高层次的用户意图
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorAction {
    // 工具切换
    SelectTranslateTool,
    SelectRotateTool,
    SelectScaleTool,
    SelectRectTool,
    
    // 相机控制
    CameraMoveForward,
    CameraMoveBackward,
    CameraMoveLeft,
    CameraMoveRight,
    CameraMoveUp,
    CameraMoveDown,
    CameraOrbit,
    CameraZoom,
    
    // 选择与操作
    SelectEntity,
    DeselectAll,
    DeleteSelected,
    DuplicateSelected,
    FocusSelected,
    
    // Gizmo 操作
    GizmoDragStart,
    GizmoDragDelta,
    GizmoDragEnd,
    
    // 场景控制
    Play,
    Pause,
    Stop,
    
    // 编辑操作
    Undo,
    Redo,
    Save,
    SaveAs,
}

/// 输入绑定配置
#[derive(Debug, Clone)]
pub struct InputBinding {
    pub action: EditorAction,
    pub context: InputContext,
    pub keys: Vec<Key>,
    pub mouse: Option<PointerButton>,
    /// 是否需要按下修饰键
    pub modifiers: InputModifiers,
    /// 触发方式
    pub trigger: TriggerType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerType {
    /// 按下瞬间触发
    Pressed,
    /// 持续按住触发
    Held,
    /// 释放时触发
    Released,
    /// 拖拽开始
    DragStart,
    /// 拖拽进行中
    DragDelta,
    /// 拖拽释放
    DragRelease,
}

/// 鼠标按钮状态
#[derive(Debug, Clone, Copy, Default)]
struct MouseButtonState {
    primary: bool,
    secondary: bool,
    middle: bool,
    extra1: bool,
    extra2: bool,
}

impl MouseButtonState {
    fn get(&self, button: PointerButton) -> bool {
        match button {
            PointerButton::Primary => self.primary,
            PointerButton::Secondary => self.secondary,
            PointerButton::Middle => self.middle,
            PointerButton::Extra1 => self.extra1,
            PointerButton::Extra2 => self.extra2,
        }
    }
    
    fn set(&mut self, button: PointerButton, value: bool) {
        match button {
            PointerButton::Primary => self.primary = value,
            PointerButton::Secondary => self.secondary = value,
            PointerButton::Middle => self.middle = value,
            PointerButton::Extra1 => self.extra1 = value,
            PointerButton::Extra2 => self.extra2 = value,
        }
    }
}

/// 编辑器输入管理器
pub struct EditorInputManager {
    /// 所有输入绑定
    bindings: Vec<InputBinding>,
    /// 当前激活的上下文
    active_contexts: Vec<InputContext>,
    /// 当前帧触发的动作
    triggered_actions: Vec<EditorAction>,
    /// 上一帧的按键状态
    prev_keys: std::collections::HashSet<Key>,
    /// 上一帧的鼠标状态
    prev_mouse_buttons: MouseButtonState,
    /// 拖拽状态
    drag_state: Option<(PointerButton, Pos2)>,
    /// 是否在视口中
    is_in_viewport: bool,
}

impl EditorInputManager {
    pub fn new() -> Self {
        let mut manager = Self {
            bindings: Vec::new(),
            active_contexts: vec![InputContext::Global],
            triggered_actions: Vec::new(),
            prev_keys: std::collections::HashSet::new(),
            prev_mouse_buttons: MouseButtonState::default(),
            drag_state: None,
            is_in_viewport: false,
        };
        
        // 注册默认绑定
        manager.register_default_bindings();
        manager
    }
    
    /// 注册默认的输入绑定
    fn register_default_bindings(&mut self) {
        // 工具切换 - 全局
        self.bindings.push(InputBinding {
            action: EditorAction::SelectTranslateTool,
            context: InputContext::Global,
            keys: vec![Key::W],
            mouse: None,
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::SelectRotateTool,
            context: InputContext::Global,
            keys: vec![Key::E],
            mouse: None,
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::SelectScaleTool,
            context: InputContext::Global,
            keys: vec![Key::R],
            mouse: None,
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        // 相机移动 - 视口内，右键按住时
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveForward,
            context: InputContext::Viewport,
            keys: vec![Key::W],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveBackward,
            context: InputContext::Viewport,
            keys: vec![Key::S],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveLeft,
            context: InputContext::Viewport,
            keys: vec![Key::A],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveRight,
            context: InputContext::Viewport,
            keys: vec![Key::D],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveDown,
            context: InputContext::Viewport,
            keys: vec![Key::Q],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::CameraMoveUp,
            context: InputContext::Viewport,
            keys: vec![Key::E],
            mouse: Some(PointerButton::Secondary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Held,
        });
        
        // Gizmo 拖拽 - 视口内，左键
        self.bindings.push(InputBinding {
            action: EditorAction::GizmoDragStart,
            context: InputContext::Viewport,
            keys: vec![],
            mouse: Some(PointerButton::Primary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::DragStart,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::GizmoDragDelta,
            context: InputContext::Viewport,
            keys: vec![],
            mouse: Some(PointerButton::Primary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::DragDelta,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::GizmoDragEnd,
            context: InputContext::Viewport,
            keys: vec![],
            mouse: Some(PointerButton::Primary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::DragRelease,
        });
        
        // 选择实体 - 视口内，左键点击
        self.bindings.push(InputBinding {
            action: EditorAction::SelectEntity,
            context: InputContext::Viewport,
            keys: vec![],
            mouse: Some(PointerButton::Primary),
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        // 删除 - 全局
        self.bindings.push(InputBinding {
            action: EditorAction::DeleteSelected,
            context: InputContext::Global,
            keys: vec![Key::Delete, Key::Backspace],
            mouse: None,
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        // 聚焦选中 - 全局
        self.bindings.push(InputBinding {
            action: EditorAction::FocusSelected,
            context: InputContext::Global,
            keys: vec![Key::F],
            mouse: None,
            modifiers: InputModifiers::default(),
            trigger: TriggerType::Pressed,
        });
        
        // Undo/Redo
        self.bindings.push(InputBinding {
            action: EditorAction::Undo,
            context: InputContext::Global,
            keys: vec![Key::Z],
            mouse: None,
            modifiers: InputModifiers { ctrl: true, shift: false, alt: false },
            trigger: TriggerType::Pressed,
        });
        
        self.bindings.push(InputBinding {
            action: EditorAction::Redo,
            context: InputContext::Global,
            keys: vec![Key::Z],
            mouse: None,
            modifiers: InputModifiers { ctrl: true, shift: true, alt: false },
            trigger: TriggerType::Pressed,
        });
        
        // 保存
        self.bindings.push(InputBinding {
            action: EditorAction::Save,
            context: InputContext::Global,
            keys: vec![Key::S],
            mouse: None,
            modifiers: InputModifiers { ctrl: true, shift: false, alt: false },
            trigger: TriggerType::Pressed,
        });
    }
    
    /// 设置当前激活的上下文
    pub fn set_context(&mut self, context: InputContext) {
        if !self.active_contexts.contains(&context) {
            self.active_contexts.push(context);
        }
    }
    
    /// 清除上下文
    pub fn clear_context(&mut self, context: InputContext) {
        self.active_contexts.retain(|c| *c != context);
    }
    
    /// 从 egui 更新输入状态
    pub fn update(&mut self, ui: &egui::Ui, viewport_hovered: bool) {
        self.triggered_actions.clear();
        self.is_in_viewport = viewport_hovered;
        
        // 更新上下文
        if viewport_hovered {
            self.set_context(InputContext::Viewport);
        } else {
            self.clear_context(InputContext::Viewport);
        }
        
        ui.input(|input| {
            // 收集当前按键状态
            let current_keys: std::collections::HashSet<Key> = input.keys_down.iter().copied().collect();
            
            // 收集当前鼠标状态
            let mut current_mouse = MouseButtonState::default();
            for button in [PointerButton::Primary, PointerButton::Secondary, 
                          PointerButton::Middle, PointerButton::Extra1, PointerButton::Extra2].iter() {
                current_mouse.set(*button, input.pointer.button_down(*button));
            }
            
            // 先处理拖拽相关的状态更新
            let mut new_drag_state = None;
            
            // 处理每个绑定
            for binding in &self.bindings {
                // 检查上下文是否激活
                if !self.active_contexts.contains(&binding.context) {
                    continue;
                }
                
                // 检查修饰键
                let mods = InputModifiers {
                    ctrl: input.modifiers.ctrl,
                    shift: input.modifiers.shift,
                    alt: input.modifiers.alt,
                };
                
                if binding.modifiers != mods {
                    continue;
                }
                
                // 检查鼠标按钮要求
                if let Some(required_button) = binding.mouse {
                    if !input.pointer.button_down(required_button) {
                        continue;
                    }
                }
                
                // 检查按键和触发类型
                let action_triggered = match binding.trigger {
                    TriggerType::Pressed => {
                        check_pressed(input, &binding.keys, binding.mouse)
                    }
                    TriggerType::Held => {
                        check_held(&current_keys, &binding.keys)
                    }
                    TriggerType::Released => {
                        check_released(&current_keys, &binding.keys, &self.prev_keys)
                    }
                    TriggerType::DragStart => {
                        let (triggered, new_state) = check_drag_start(input, binding.mouse, &self.drag_state);
                        if triggered {
                            new_drag_state = new_state;
                        }
                        triggered
                    }
                    TriggerType::DragDelta => {
                        check_drag_delta(input, binding.mouse, &self.drag_state)
                    }
                    TriggerType::DragRelease => {
                        let (triggered, new_state) = check_drag_release(input, binding.mouse, &self.drag_state);
                        if triggered {
                            new_drag_state = new_state;
                        }
                        triggered
                    }
                };
                
                if action_triggered {
                    self.triggered_actions.push(binding.action);
                }
            }
            
            // 更新拖拽状态
            if let Some(new_state) = new_drag_state {
                self.drag_state = Some(new_state);
            }
            
            // 更新上一帧状态
            self.prev_keys = current_keys;
            self.prev_mouse_buttons = current_mouse;
        });
    }
    
    /// 获取当前帧触发的动作
    pub fn triggered_actions(&self) -> &[EditorAction] {
        &self.triggered_actions
    }
    
    /// 检查特定动作是否被触发
    pub fn is_action_triggered(&self, action: EditorAction) -> bool {
        self.triggered_actions.contains(&action)
    }
    
    /// 获取鼠标增量（用于相机旋转）
    pub fn mouse_delta(&self, ui: &egui::Ui) -> egui::Vec2 {
        ui.input(|i| i.pointer.delta())
    }
    
    /// 获取滚轮增量（用于缩放）
    pub fn scroll_delta(&self, ui: &egui::Ui) -> f32 {
        ui.input(|i| i.raw_scroll_delta.y)
    }
    
    /// 检查是否在拖拽
    pub fn is_dragging(&self, button: PointerButton) -> bool {
        self.drag_state.map_or(false, |(b, _)| b == button)
    }
    
    /// 获取拖拽增量
    pub fn drag_delta(&self, ui: &egui::Ui) -> Option<egui::Vec2> {
        if self.drag_state.is_some() {
            Some(ui.input(|i| i.pointer.delta()))
        } else {
            None
        }
    }
    
    /// 获取当前拖拽状态
    pub fn drag_state(&self) -> Option<(PointerButton, Pos2)> {
        self.drag_state
    }
    
    /// 获取拖拽起始位置（转换为 Vec2）
    pub fn drag_start_pos_vec2(&self) -> Option<egui::Vec2> {
        self.drag_state.map(|(_, pos)| pos.to_vec2())
    }
    
    /// 检查鼠标按钮是否刚按下
    pub fn is_mouse_clicked(&self, ui: &egui::Ui, button: PointerButton) -> bool {
        ui.input(|i| i.pointer.button_clicked(button))
    }
    
    /// 检查鼠标按钮是否刚释放
    pub fn is_mouse_released(&self, ui: &egui::Ui, button: PointerButton) -> bool {
        ui.input(|i| i.pointer.button_released(button))
    }
    
    /// 重置所有状态
    pub fn reset(&mut self) {
        self.triggered_actions.clear();
        self.drag_state = None;
    }
}

// 独立的辅助函数，避免借用冲突

/// 检查按键是否刚按下
fn check_pressed(
    input: &egui::InputState,
    binding_keys: &[Key],
    required_mouse: Option<PointerButton>,
) -> bool {
    // 如果要求鼠标按钮，检查鼠标是否也刚按下
    if let Some(button) = required_mouse {
        if !input.pointer.button_clicked(button) {
            return false;
        }
    }
    
    // 检查按键是否刚按下
    for key in binding_keys {
        if input.key_pressed(*key) {
            return true;
        }
    }
    
    // 如果没有按键要求，只检查鼠标
    if binding_keys.is_empty() && required_mouse.is_some() {
        return true;
    }
    
    false
}

/// 检查按键是否持续按住
fn check_held(
    current_keys: &std::collections::HashSet<Key>,
    binding_keys: &[Key],
) -> bool {
    if binding_keys.is_empty() {
        return true;
    }
    binding_keys.iter().any(|key| current_keys.contains(key))
}

/// 检查按键是否刚释放
fn check_released(
    current_keys: &std::collections::HashSet<Key>,
    binding_keys: &[Key],
    prev_keys: &std::collections::HashSet<Key>,
) -> bool {
    binding_keys.iter().any(|key| {
        !current_keys.contains(key) && prev_keys.contains(key)
    })
}

/// 检查拖拽是否开始
fn check_drag_start(
    input: &egui::InputState,
    mouse_button: Option<PointerButton>,
    current_drag_state: &Option<(PointerButton, Pos2)>,
) -> (bool, Option<(PointerButton, Pos2)>) {
    if let Some(button) = mouse_button {
        if input.pointer.button_clicked(button) {
            if let Some(pos) = input.pointer.interact_pos() {
                return (true, Some((button, pos)));
            }
        }
    }
    (false, None)
}

/// 检查拖拽增量
fn check_drag_delta(
    input: &egui::InputState,
    mouse_button: Option<PointerButton>,
    current_drag_state: &Option<(PointerButton, Pos2)>,
) -> bool {
    if let Some(button) = mouse_button {
        if let Some((drag_button, _)) = current_drag_state {
            if *drag_button == button && input.pointer.button_down(button) {
                let delta = input.pointer.delta();
                return delta != egui::Vec2::ZERO;
            }
        }
    }
    false
}

/// 检查拖拽是否释放
fn check_drag_release(
    input: &egui::InputState,
    mouse_button: Option<PointerButton>,
    current_drag_state: &Option<(PointerButton, Pos2)>,
) -> (bool, Option<(PointerButton, Pos2)>) {
    if let Some(button) = mouse_button {
        if let Some((drag_button, _)) = current_drag_state {
            if *drag_button == button && input.pointer.button_released(button) {
                return (true, None);
            }
        }
    }
    (false, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_editor_input_manager_creation() {
        let manager = EditorInputManager::new();
        assert!(!manager.bindings.is_empty());
        assert!(manager.active_contexts.contains(&InputContext::Global));
    }
    
    #[test]
    fn test_context_management() {
        let mut manager = EditorInputManager::new();
        
        manager.set_context(InputContext::Viewport);
        assert!(manager.active_contexts.contains(&InputContext::Viewport));
        
        manager.clear_context(InputContext::Viewport);
        assert!(!manager.active_contexts.contains(&InputContext::Viewport));
    }
    
    #[test]
    fn test_triggered_actions_empty_initially() {
        let manager = EditorInputManager::new();
        assert!(manager.triggered_actions().is_empty());
    }
    
    #[test]
    fn test_mouse_button_state() {
        let mut state = MouseButtonState::default();
        
        assert!(!state.get(PointerButton::Primary));
        
        state.set(PointerButton::Primary, true);
        assert!(state.get(PointerButton::Primary));
        
        state.set(PointerButton::Primary, false);
        assert!(!state.get(PointerButton::Primary));
    }
}