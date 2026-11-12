# Cozy project data

These terms define ownership of a project's contents and execution history.

## Language

**Project data**:
The contents, settings, activities and history belonging to one project, including its run records, inputs, outputs, diagnostics and derived data.
_Avoid_: Global run history, controller-owned project history

**Project reference**:
The opaque identity and storage location needed to find a project. It contains no project content, activity or execution history.
_Avoid_: Project summary, recent-project history

**Project run**:
An execution belonging to one project, together with the record of its inputs, execution and results.
_Avoid_: Global run
