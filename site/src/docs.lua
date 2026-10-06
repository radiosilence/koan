-- The markdown in docs/ is written to read on GitHub as well: one `# Title`
-- heading, and links between files by relative path. On the site the title
-- belongs to the template, and each file is a page at /docs/<name>/.

function Pandoc(doc)
  local first = doc.blocks[1]
  if first and first.t == "Header" and first.level == 1 then
    doc.meta.title = pandoc.Inlines(first.content)
    doc.blocks:remove(1)
  end
  return doc
end

function Link(el)
  if el.target:match("^%a[%w+.-]*:") then
    return el
  end
  local name, anchor = el.target:match("([%w_-]+)%.md(#?.*)$")
  if name then
    el.target = "/docs/" .. name:lower() .. "/" .. anchor
  end
  return el
end

-- Pictures are written as paths into the site's public folder, so GitHub shows
-- them too; on the site that folder is the root.
function Image(el)
  local path = el.src:match("site/public(/.*)$")
  if path then
    el.src = path
  end
  return el
end
