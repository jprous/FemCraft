"""Minimal SOAP client for a running RFEM 6 (API I, document/literal).

Only what the library extractor needs: call(method, **xml_args) -> parsed dict.
"""
import re
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

import requests

ENV = "http://schemas.xmlsoap.org/soap/envelope/"


def to_xml(name, value):
    if isinstance(value, dict):
        inner = "".join(to_xml(k, v) for k, v in value.items() if v is not None)
        return f"<{name}>{inner}</{name}>"
    if isinstance(value, bool):
        value = "true" if value else "false"
    return f"<{name}>{escape(str(value))}</{name}>"


def parse(el):
    kids = list(el)
    if not kids:
        t = el.text
        if t is None:
            return None
        for conv in (int, float):
            try:
                return conv(t)
            except ValueError:
                pass
        if t in ("true", "false"):
            return t == "true"
        return t
    out = {}
    for k in kids:
        tag = k.tag.split("}")[-1]
        v = parse(k)
        if tag in out:
            if not isinstance(out[tag], list):
                out[tag] = [out[tag]]
            out[tag].append(v)
        else:
            out[tag] = v
    return out


class SoapFault(Exception):
    pass


class Client:
    def __init__(self, url):
        self.url = url.rstrip("/") + "/"
        self.session = requests.Session()
        wsdl = self.session.get(self.url + "wsdl", timeout=60).text
        self.ns = re.search(r'definitions[^>]*targetNamespace="([^"]+)"', wsdl).group(1)

    def call(self, method, **args):
        body = "".join(to_xml(k, v) for k, v in args.items())
        xml = (f'<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="{ENV}">'
               f'<soap:Body><{method} xmlns="http://www.dlubal.com/rfem.xsd">{body}</{method}>'
               f"</soap:Body></soap:Envelope>")
        r = self.session.post(self.url, data=xml.encode("utf-8"), timeout=300, headers={
            "Content-Type": "text/xml; charset=utf-8",
            "SOAPAction": self.url.replace("127.0.0.1", "localhost") + method})
        root = ET.fromstring(r.content)
        fault = root.find(f".//{{{ENV}}}Fault")
        if fault is not None:
            raise SoapFault(" ".join(fault.itertext()).strip())
        resp = root.find(f"{{{ENV}}}Body")[0]
        return parse(resp)
