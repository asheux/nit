; Dockerfile highlights (minimal, editor-grade)
(comment) @comment

(double_quoted_string) @string
(unquoted_string) @string

(image_spec) @type
(image_name) @type
(image_tag) @string.special
(image_digest) @string.special

[
  "FROM"
  "RUN"
  "CMD"
  "LABEL"
  "MAINTAINER"
  "EXPOSE"
  "ENV"
  "ADD"
  "COPY"
  "ENTRYPOINT"
  "VOLUME"
  "USER"
  "WORKDIR"
  "ARG"
  "ONBUILD"
  "STOPSIGNAL"
  "HEALTHCHECK"
  "SHELL"
  "AS"
] @keyword
