_G.test_log = {}

get.ping = function(self)
    return "pong"
end

get.echo = function(self)
    return { status = 200, body = { query = self.query, headers = self.headers } }
end

post.data = function(self)
    return { status = 201, body = { received = json_decode(self.body) } }
end

put.replace = function(self)
    return { status = 200, body = "replaced" }
end

patch.partial = function(self)
    return { status = 200, body = "patched" }
end

delete.remove = function(self)
    return { status = 204 }
end

all.catchall = function(self)
    return { status = 200, body = { method = self.method, path = self.path } }
end

get.user = function(self)
    return { status = 200, body = { user_id = self.path_args[1] } }
end

get.deferred = function(self)
    defer(function(ctx)
        table.insert(_G.test_log, "server_defer_ran")
    end)
    defer(function()
        table.insert(_G.test_log, "server_defer_self:" .. self.path)
        global_store_set("test_defer_self_path", self.path)
    end)
    return "deferred"
end

get.auth_check = function(self)
    return { status = 200, body = "authenticated" }
end

-- Sync binding tests
get.json_encode_test = function(self)
    local encoded = json_encode({ a = 1, b = "two", c = true })
    return { status = 200, body = encoded }
end

get.json_decode_test = function(self)
    local decoded = json_decode('{"x": 42, "y": "hello"}')
    return { status = 200, body = { x = decoded.x, y = decoded.y } }
end

get.env_get_test = function(self)
    local val = env_get("SPYWEB_TEST_VAR")
    return { status = 200, body = { value = val } }
end

get.dump_test = function(self)
    local result = dump({ x = 1, nested = { a = true } })
    return { status = 200, body = result }
end

get.copy_test = function(self)
    local original = { a = 1, b = 2 }
    local copied = copy(original)
    copied.a = 99
    return { status = 200, body = { original_a = original.a, copied_a = copied.a } }
end

get.deep_copy_test = function(self)
    local original = { nested = { val = 10 } }
    local copied = deep_copy(original)
    copied.nested.val = 99
    return { status = 200, body = { original_val = original.nested.val, copied_val = copied.nested.val } }
end
