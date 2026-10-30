# Panelink workspace

Panelink organizes ongoing terminal work into projects, workspace tabs and panes.

## Language

**Project**:
A named group of workspace tabs.

**Workspace tab**:
A workspace containing its own pane layout, presented as a tab within a project.
In UI discussions, “tab” is shorthand for workspace tab.
_Avoid_: Pane, host terminal tab

**Pane**:
An individual terminal area within a workspace tab, containing a shell or another
terminal application.
_Avoid_: Tab, agent

**Focused pane**:
The pane currently receiving keyboard input within the active workspace tab.
Hovering, scrolling or previewing a drop destination does not itself make that
pane focused.
_Avoid_: Selected pane when referring to text selection or a drop destination

**Close pane**:
End the process in a pane and remove that pane after confirmation.
_Avoid_: Hide pane, detach pane

**Close workspace tab**:
End the processes in a workspace tab and remove that tab after confirmation.
_Avoid_: Hide tab, detach tab

**Detach**:
Leave a running workspace without ending its panes or their processes.
_Avoid_: Close, stop
