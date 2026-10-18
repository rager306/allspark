# Reference

Both methods have the same body today, so `sed 's/retries={} timeout={}/.../'` hits
both. The symbol layer addresses one by name path (`Config/describe`), which is why
`replace_symbol_body` is the correct tool here.
