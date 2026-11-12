# syntax=docker/dockerfile:1

# Build the real TeleDB-DBProxy (UDAL) from its open-source repository. The source is not
# vendored: the clone is pinned to the commit this lab was verified against.
FROM maven:3.9-eclipse-temurin-8 AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*

RUN git clone https://gitee.com/teledb/teledb-dbproxy.git /src \
    && cd /src \
    && git checkout 1c05326e955f97675f80f93efac14903aee3bf45

WORKDIR /src
# The open-source DBProxy hard-codes CREATE/DROP/ALTER as forbidden
# (`PrivilegePoints.forbid()`). Commercial UDAL may allow DDL; this opt-in patch lets the lab
# exercise the app's table designer. It is deliberately off by default.
ARG UDAL_ALLOW_DDL=0
RUN if [ "$UDAL_ALLOW_DDL" = "1" ]; then \
      sed -i 's/return NONE_MASK == mask;/return false; \/\/ RustGrid lab: allow DDL/' \
        src/main/java/com/ctg/udal/dbproxy/configloader/model/PrivilegeConfig.java; \
    fi
COPY m2-settings.xml /tmp/m2-settings.xml
RUN mvn -B -s /tmp/m2-settings.xml -DskipTests package \
    && mkdir -p /opt \
    && tar xzf target/ctg-udal-dbproxy-2.7.5.tar.gz -C /opt

FROM eclipse-temurin:8-jre

WORKDIR /opt/dbproxy
COPY --from=build /opt/DBProxy /opt/dbproxy
COPY config.json myid.properties cacheservice.properties /opt/dbproxy/conf/
RUN mkdir -p /opt/dbproxy/logs

EXPOSE 8066
CMD ["java", \
     "-DDBPROXY_HOME=/opt/dbproxy", \
     "-DLOG_BASE_DIR=/opt/dbproxy", \
     "-Dddl.config.type=file", \
     "-Dddl.config.cluster=dbproxy_cluster_0000000000", \
     "-Dddl.config.node=node_0000000000", \
     "-cp", "conf:lib/*", \
     "com.ctg.udal.dbproxy.DBProxyStartup"]
