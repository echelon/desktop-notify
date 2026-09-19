on run argv
  set mode to item 1 of argv
  set wanted to item 2 of argv
  tell application "Ghostty"
    if mode is "terminal" then
      set matches to every terminal whose id is wanted
      if (count of matches) is 1 then
        focus item 1 of matches
        activate
        return "focused"
      end if
    else if mode is "window" or mode is "title" then
      set matches to {}
      repeat with w in windows
        if (mode is "window" and (id of w as text) is wanted) or (mode is "title" and name of w is wanted) then set end of matches to w
      end repeat
      if (count of matches) is 1 then
        activate window (item 1 of matches)
        activate
        return "focused"
      end if
    end if
  end tell
  return "missing"
end run
