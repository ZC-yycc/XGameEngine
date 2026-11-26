#include <stddef.h>

/* 只声明 vendored Lua 5.4 的两个入口。Lua 的错误跳转须发生在 Rust 计数回调返回后。 */
typedef struct lua_State lua_State;
typedef struct lua_Debug lua_Debug;
extern const char *lua_pushlstring(lua_State *, const char *, size_t);
extern int lua_error(lua_State *);
extern int xgc_lua_budget_tick(lua_State *, lua_Debug *);

void xgc_lua_instruction_hook(lua_State *state, lua_Debug *event) {
    static const char message[] = "instruction budget exceeded";
    if (xgc_lua_budget_tick(state, event)) {
        lua_pushlstring(state, message, sizeof(message) - 1);
        lua_error(state);
    }
}
