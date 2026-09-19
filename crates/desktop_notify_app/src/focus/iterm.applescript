on run argv
  set mode to item 1 of argv
  set wanted to item 2 of argv
  tell application "iTerm2"
    set matches to {}
    repeat with w in windows
      if mode is "terminal" or mode is "tty" then
        repeat with t in tabs of w
          repeat with s in sessions of t
            if (mode is "terminal" and unique id of s is wanted) or (mode is "tty" and tty of s is wanted) then set end of matches to {w, t, s}
          end repeat
        end repeat
      else if (mode is "window" and (id of w as text) is wanted) or (mode is "title" and name of w is wanted) then
        set end of matches to {w}
      end if
    end repeat
    if (count of matches) is 1 then
      set matched to item 1 of matches
      if (count of matched) is 3 then
        select item 3 of matched
        select item 2 of matched
      end if
      select item 1 of matched
      activate
      return "focused"
    end if
  end tell
  return "missing"
end run
