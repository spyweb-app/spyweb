-- Intercept the request to route through an external rendering service
function before_fetch(request)
    local target_url = request.url
    
    -- Redirect the request to your preferred rendering API
    request.url = "https://api.rendering-service.com/render?token=YOUR_API_KEY"
    request.method = "POST"
    request.headers["Content-Type"] = "application/json"
    
    -- Configure the service to load the target URL and wait for JS to execute
    request.body = json_encode({
        url = target_url,
        wait_until = "network_idle"
    })
    
    -- spyweb will now fetch the fully rendered HTML from the provider!
    return request
end
