_G.log = {}
sleep(5)

function before_fetch(request, ctx)
    -- require from job directory
    local local_mod = require("local_module")
    table.insert(_G.log, "local:" .. tostring(local_mod.val))

    -- require with deep dotted path
    local deep_mod = require("deep.child.deep_module")
    table.insert(_G.log, "deep:" .. tostring(deep_mod.val))

    -- require with init.lua (directory module)
    local dir_mod = require("dir_module")
    table.insert(_G.log, "dir:" .. tostring(dir_mod.val))

    -- require from project root (fallback, optional)
    local root_ok, root_mod = pcall(require, "root_module")
    if root_ok then
        table.insert(_G.log, "root:" .. tostring(root_mod.val))
    end

    -- security: traversal
    local ok1, err1 = pcall(require, "../../etc/passwd")
    table.insert(_G.log, ok1 and "BREACH:traversal" or "blocked:traversal")

    -- security: absolute path
    local ok2, err2 = pcall(require, "/etc/passwd")
    table.insert(_G.log, ok2 and "BREACH:absolute" or "blocked:absolute")

    -- security: dot-dot
    local ok3, err3 = pcall(require, "../config")
    table.insert(_G.log, ok3 and "BREACH:dotdot" or "blocked:dotdot")

    -- security: double dot-dot
    local ok4, err4 = pcall(require, "../../Cargo")
    table.insert(_G.log, ok4 and "BREACH:doubledot" or "blocked:doubledot")

    -- security: null byte
    local ok5, err5 = pcall(require, "legit\0/../etc/passwd")
    table.insert(_G.log, ok5 and "BREACH:nullbyte" or "blocked:nullbyte")

    -- security: symlink (escape_link.lua created by test if it exists)
    -- local ok6 = require("escape_link")
    local ok6, err6 = pcall(require, "escape_link")
    table.insert(_G.log, ok6 and "BREACH:symlink" or "blocked:symlink")

    _G.require_tested = true
    return request
end

function override_fetch(request, ctx)
    return { status = 200, body = "<html>ok</html>", url = request.url }
end
